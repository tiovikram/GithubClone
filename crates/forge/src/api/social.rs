//! Users extras: GPG keys and followers/following.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/gpg_keys", get(list_my_gpg).post(create_gpg))
        .route("/user/gpg_keys/{id}", get(get_gpg).delete(delete_gpg))
        .route("/users/{username}/gpg_keys", get(list_user_gpg))
        .route("/user/followers", get(my_followers))
        .route("/user/following", get(my_following))
        .route("/user/following/{username}", get(am_following).put(follow).delete(unfollow))
        .route("/users/{username}/followers", get(user_followers))
        .route("/users/{username}/following", get(user_following))
        .route("/users/{username}/following/{target}", get(user_follows))
}

// ---------------------------------------------------------------------------------------------
// OpenPGP (minimal): dearmor, walk packets, compute key ids, collect user-id emails.

#[derive(Debug, Clone)]
pub struct PgpKey {
    pub key_id: String,
    pub packet_b64: String,
    pub created: Option<DateTime<Utc>>,
    pub algo: u8,
}

#[derive(Debug, Clone)]
pub struct ParsedGpg {
    pub primary: PgpKey,
    pub subkeys: Vec<PgpKey>,
    pub emails: Vec<String>,
}

fn dearmor(armored: &str) -> Option<Vec<u8>> {
    let text = armored.replace("\r\n", "\n");
    let start = text.find("-----BEGIN PGP PUBLIC KEY BLOCK-----")?;
    let rest = &text[start..];
    let mut lines = rest.lines().skip(1);
    // Armor headers end at the first blank line (headers are optional).
    let mut body = String::new();
    let mut in_headers = true;
    for line in lines.by_ref() {
        let l = line.trim();
        if l.starts_with("-----END") {
            break;
        }
        if in_headers {
            if l.is_empty() {
                in_headers = false;
                continue;
            }
            if l.contains(": ") {
                continue;
            }
            in_headers = false;
        }
        if l.starts_with('=') && l.len() <= 5 {
            continue; // CRC24 checksum
        }
        body.push_str(l);
    }
    base64::engine::general_purpose::STANDARD.decode(body.as_bytes()).ok()
}

/// Returns (tag, header+body bytes, body bytes) for each packet.
fn packets(data: &[u8]) -> Option<Vec<(u8, &[u8], &[u8])>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let start = i;
        let b = data[i];
        if b & 0x80 == 0 {
            return None;
        }
        i += 1;
        let (tag, len) = if b & 0x40 != 0 {
            let tag = b & 0x3f;
            let l1 = *data.get(i)? as usize;
            i += 1;
            let len = if l1 < 192 {
                l1
            } else if l1 < 224 {
                let l2 = *data.get(i)? as usize;
                i += 1;
                ((l1 - 192) << 8) + l2 + 192
            } else if l1 == 255 {
                let v = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize;
                i += 4;
                v
            } else {
                return None; // partial lengths don't occur in key packets
            };
            (tag, len)
        } else {
            let tag = (b >> 2) & 0x0f;
            let len = match b & 3 {
                0 => {
                    let v = *data.get(i)? as usize;
                    i += 1;
                    v
                }
                1 => {
                    let v = u16::from_be_bytes(data.get(i..i + 2)?.try_into().ok()?) as usize;
                    i += 2;
                    v
                }
                2 => {
                    let v = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize;
                    i += 4;
                    v
                }
                _ => data.len() - i,
            };
            (tag, len)
        };
        let body = data.get(i..i + len)?;
        out.push((tag, &data[start..i + len], body));
        i += len;
    }
    Some(out)
}

fn key_from_packet(packet: &[u8], body: &[u8]) -> Option<PgpKey> {
    let version = *body.first()?;
    let created = u32::from_be_bytes(body.get(1..5)?.try_into().ok()?);
    let (key_id, algo) = match version {
        4 => {
            let mut h = Sha1::new();
            h.update([0x99]);
            h.update((body.len() as u16).to_be_bytes());
            h.update(body);
            let fp = h.finalize();
            (hex::encode_upper(&fp[12..20]), *body.get(5)?)
        }
        5 | 6 => {
            let mut h = Sha256::new();
            h.update([if version == 5 { 0x9a } else { 0x9b }]);
            h.update((body.len() as u32).to_be_bytes());
            h.update(body);
            let fp = h.finalize();
            (hex::encode_upper(&fp[0..8]), *body.get(5)?)
        }
        _ => return None,
    };
    Some(PgpKey {
        key_id,
        packet_b64: base64::engine::general_purpose::STANDARD.encode(packet),
        created: Utc.timestamp_opt(created as i64, 0).single(),
        algo,
    })
}

/// Parses an ASCII-armored OpenPGP public key: primary key, subkeys and user-id emails.
pub fn parse_gpg(armored: &str) -> Option<ParsedGpg> {
    let data = dearmor(armored)?;
    let pk = packets(&data)?;
    let mut primary = None;
    let mut subkeys = Vec::new();
    let mut emails = Vec::new();
    for (tag, packet, body) in pk {
        match tag {
            6 if primary.is_none() => primary = Some(key_from_packet(packet, body)?),
            14 => subkeys.push(key_from_packet(packet, body)?),
            13 => {
                let uid = String::from_utf8_lossy(body);
                if let (Some(a), Some(b)) = (uid.find('<'), uid.rfind('>')) {
                    if a < b {
                        emails.push(uid[a + 1..b].to_string());
                    }
                } else if uid.contains('@') {
                    emails.push(uid.trim().to_string());
                }
            }
            _ => {}
        }
    }
    Some(ParsedGpg { primary: primary?, subkeys, emails })
}

/// (can_sign, can_encrypt, can_certify) from the public-key algorithm.
fn caps(algo: u8, primary: bool) -> (bool, bool, bool) {
    match algo {
        1 => (primary, !primary, primary),
        2 | 16 | 18 | 25 | 26 => (false, true, false),
        3 | 17 | 19 | 22 | 27 | 28 => (true, false, primary),
        _ => (primary, !primary, primary),
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GpgRow {
    pub id: i64,
    pub user_id: i64,
    pub primary_key_id: Option<i64>,
    pub name: Option<String>,
    pub key_id: String,
    pub public_key: String,
    pub raw_key: Option<String>,
    pub emails: Vec<String>,
    pub can_sign: bool,
    pub can_encrypt_comms: bool,
    pub can_encrypt_storage: bool,
    pub can_certify: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

fn gpg_json(k: &GpgRow, subkeys: &[&GpgRow], verified_email: Option<&str>) -> Value {
    let emails: Vec<Value> = k
        .emails
        .iter()
        .map(|e| json!({"email": e, "verified": verified_email.is_some_and(|v| v.eq_ignore_ascii_case(e))}))
        .collect();
    let mut v = json!({
        "id": k.id,
        "name": k.name,
        "primary_key_id": k.primary_key_id,
        "key_id": k.key_id,
        "public_key": k.public_key,
        "emails": emails,
        "subkeys": subkeys.iter().map(|s| gpg_json(s, &[], None)).collect::<Vec<_>>(),
        "can_sign": k.can_sign,
        "can_encrypt_comms": k.can_encrypt_comms,
        "can_encrypt_storage": k.can_encrypt_storage,
        "can_certify": k.can_certify,
        "created_at": crate::json::ts(k.created_at),
        "expires_at": crate::json::ts_opt(k.expires_at),
        "revoked": false,
        "raw_key": k.raw_key,
    });
    if k.primary_key_id.is_some() {
        v.as_object_mut().unwrap().remove("name");
        v["raw_key"] = Value::Null;
    }
    v
}

async fn gpg_list_json(s: &AppState, user: &User, p: PageParams) -> ApiResult<(Vec<Value>, i64)> {
    let prim: Vec<GpgRow> = sqlx::query_as(
        "SELECT * FROM gpg_keys WHERE user_id = $1 AND primary_key_id IS NULL ORDER BY id LIMIT $2 OFFSET $3",
    )
    .bind(user.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM gpg_keys WHERE user_id = $1 AND primary_key_id IS NULL")
        .bind(user.id)
        .fetch_one(&s.db)
        .await?;
    let ids: Vec<i64> = prim.iter().map(|k| k.id).collect();
    let subs: Vec<GpgRow> = sqlx::query_as("SELECT * FROM gpg_keys WHERE primary_key_id = ANY($1) ORDER BY id")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    Ok((
        prim.iter()
            .map(|k| {
                let mine: Vec<&GpgRow> = subs.iter().filter(|x| x.primary_key_id == Some(k.id)).collect();
                gpg_json(k, &mine, user.email.as_deref())
            })
            .collect(),
        total,
    ))
}

#[derive(Deserialize)]
pub struct CreateGpg {
    pub name: Option<String>,
    pub armored_public_key: Option<String>,
}

fn gpg_invalid(msg: &str) -> ApiError {
    let mut e = ApiError::validation("GpgKey", "armored_public_key", "custom");
    e.errors = Some(json!([{"resource": "GpgKey", "code": "custom", "field": "armored_public_key", "message": msg}]));
    e
}

/// Adds a GPG key (and its subkeys) for `user`.
pub async fn create_gpg_key(s: &AppState, user: &User, req: CreateGpg) -> ApiResult<Value> {
    let armored = req.armored_public_key.filter(|a| !a.trim().is_empty()).ok_or_else(|| ApiError::validation("GpgKey", "armored_public_key", "missing_field"))?;
    let parsed = parse_gpg(&armored).ok_or_else(|| gpg_invalid("We got an error doing that."))?;
    let (sign, enc, cert) = caps(parsed.primary.algo, true);
    let mut tx = s.db.begin().await?;
    let prim: Option<GpgRow> = sqlx::query_as(
        "INSERT INTO gpg_keys (user_id, name, key_id, public_key, raw_key, emails, can_sign, can_encrypt_comms,
            can_encrypt_storage, can_certify, created_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$9, COALESCE($10, now())) ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(user.id)
    .bind(req.name.filter(|n| !n.is_empty()))
    .bind(&parsed.primary.key_id)
    .bind(&parsed.primary.packet_b64)
    .bind(&armored)
    .bind(&parsed.emails)
    .bind(sign)
    .bind(enc)
    .bind(cert)
    .bind(parsed.primary.created)
    .fetch_optional(&mut *tx)
    .await?;
    let prim = prim.ok_or_else(|| {
        let mut e = ApiError::validation("GpgKey", "key_id", "custom");
        e.errors = Some(json!([{"resource": "GpgKey", "code": "custom", "field": "key_id", "message": "key_id already exists"}]));
        e
    })?;
    for sk in &parsed.subkeys {
        let (sign, enc, cert) = caps(sk.algo, false);
        sqlx::query(
            "INSERT INTO gpg_keys (user_id, primary_key_id, key_id, public_key, can_sign, can_encrypt_comms,
                can_encrypt_storage, can_certify, created_at)
             VALUES ($1,$2,$3,$4,$5,$6,$6,$7, COALESCE($8, now()))",
        )
        .bind(user.id)
        .bind(prim.id)
        .bind(&sk.key_id)
        .bind(&sk.packet_b64)
        .bind(sign)
        .bind(enc)
        .bind(cert)
        .bind(sk.created)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    let subs: Vec<GpgRow> = sqlx::query_as("SELECT * FROM gpg_keys WHERE primary_key_id = $1 ORDER BY id").bind(prim.id).fetch_all(&s.db).await?;
    let refs: Vec<&GpgRow> = subs.iter().collect();
    Ok(gpg_json(&prim, &refs, user.email.as_deref()))
}

async fn list_my_gpg(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (items, total) = gpg_list_json(&s, &u, p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn list_user_gpg(State(s): State<AppState>, uri: OriginalUri, Path(login): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    let (items, total) = gpg_list_json(&s, &u, p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create_gpg(State(s): State<AppState>, AuthUser(u): AuthUser, Json(req): Json<CreateGpg>) -> ApiResult<Response> {
    let v = create_gpg_key(&s, &u, req).await?;
    Ok(created(v))
}

async fn get_gpg(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let k: GpgRow = sqlx::query_as("SELECT * FROM gpg_keys WHERE id = $1 AND user_id = $2 AND primary_key_id IS NULL")
        .bind(id)
        .bind(u.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let subs: Vec<GpgRow> = sqlx::query_as("SELECT * FROM gpg_keys WHERE primary_key_id = $1 ORDER BY id").bind(id).fetch_all(&s.db).await?;
    let refs: Vec<&GpgRow> = subs.iter().collect();
    Ok(Json(gpg_json(&k, &refs, u.email.as_deref())))
}

async fn delete_gpg(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let n = sqlx::query("DELETE FROM gpg_keys WHERE id = $1 AND user_id = $2 AND primary_key_id IS NULL")
        .bind(id)
        .bind(u.id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Followers

/// (followers, following) counts for a user.
pub async fn follow_counts(db: &sqlx::PgPool, user_id: i64) -> ApiResult<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM follows WHERE followee_id = $1), (SELECT count(*) FROM follows WHERE follower_id = $1)",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

async fn users_page(s: &AppState, sql: &str, id: i64, p: PageParams) -> ApiResult<Vec<Value>> {
    let users: Vec<User> = sqlx::query_as(sqlx::AssertSqlSafe(sql.to_string()))
        .bind(id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    Ok(users.iter().map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).collect())
}

const FOLLOWERS_SQL: &str = "SELECT u.* FROM follows f JOIN users u ON u.id = f.follower_id WHERE f.followee_id = $1 ORDER BY f.created_at, u.id LIMIT $2 OFFSET $3";
const FOLLOWING_SQL: &str = "SELECT u.* FROM follows f JOIN users u ON u.id = f.followee_id WHERE f.follower_id = $1 ORDER BY f.created_at, u.id LIMIT $2 OFFSET $3";

async fn my_followers(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let items = users_page(&s, FOLLOWERS_SQL, u.id, p).await?;
    let (total, _) = follow_counts(&s.db, u.id).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn my_following(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let items = users_page(&s, FOLLOWING_SQL, u.id, p).await?;
    let (_, total) = follow_counts(&s.db, u.id).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn user_followers(State(s): State<AppState>, uri: OriginalUri, Path(login): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    let items = users_page(&s, FOLLOWERS_SQL, u.id, p).await?;
    let (total, _) = follow_counts(&s.db, u.id).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn user_following(State(s): State<AppState>, uri: OriginalUri, Path(login): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    let items = users_page(&s, FOLLOWING_SQL, u.id, p).await?;
    let (_, total) = follow_counts(&s.db, u.id).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn is_following(db: &sqlx::PgPool, a: i64, b: i64) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM follows WHERE follower_id = $1 AND followee_id = $2)")
        .bind(a)
        .bind(b)
        .fetch_one(db)
        .await?)
}

async fn am_following(State(s): State<AppState>, AuthUser(u): AuthUser, Path(login): Path<String>) -> ApiResult<Response> {
    let t = db::user_by_login(&s.db, &login).await?;
    if is_following(&s.db, u.id, t.id).await? {
        Ok(no_content())
    } else {
        Err(ApiError::not_found())
    }
}

async fn user_follows(State(s): State<AppState>, _u: MaybeUser, Path((login, target)): Path<(String, String)>) -> ApiResult<Response> {
    let a = db::user_by_login(&s.db, &login).await?;
    let b = db::user_by_login(&s.db, &target).await?;
    if is_following(&s.db, a.id, b.id).await? {
        Ok(no_content())
    } else {
        Err(ApiError::not_found())
    }
}

pub async fn follow_user(s: &AppState, follower: &User, target: &User) -> ApiResult<()> {
    if follower.id == target.id {
        return Ok(());
    }
    sqlx::query("INSERT INTO follows (follower_id, followee_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(follower.id)
        .bind(target.id)
        .execute(&s.db)
        .await?;
    Ok(())
}

async fn follow(State(s): State<AppState>, AuthUser(u): AuthUser, Path(login): Path<String>) -> ApiResult<Response> {
    let t = db::user_by_login(&s.db, &login).await?;
    follow_user(&s, &u, &t).await?;
    Ok(no_content())
}

async fn unfollow(State(s): State<AppState>, AuthUser(u): AuthUser, Path(login): Path<String>) -> ApiResult<Response> {
    let t = db::user_by_login(&s.db, &login).await?;
    sqlx::query("DELETE FROM follows WHERE follower_id = $1 AND followee_id = $2")
        .bind(u.id)
        .bind(t.id)
        .execute(&s.db)
        .await?;
    Ok(no_content())
}

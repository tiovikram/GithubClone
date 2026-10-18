//! Repository statistics (`/stats/*`, computed from the default branch history), the ref
//! activity log (`GET /repos/{o}/{r}/activity`) and the repos area's ref-change hook
//! ([`on_refs_changed`]: activity, rule suites, Pages builds).
use std::collections::{BTreeMap, HashMap};

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::MaybeUser;
use crate::db::{self, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;
use crate::webhooks::ZERO_SHA;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}";
    Router::new()
        .route(&format!("{r}/stats/code_frequency"), get(code_frequency))
        .route(&format!("{r}/stats/commit_activity"), get(commit_activity))
        .route(&format!("{r}/stats/contributors"), get(contributors))
        .route(&format!("{r}/stats/participation"), get(participation))
        .route(&format!("{r}/stats/punch_card"), get(punch_card))
        .route(&format!("{r}/activity"), get(activity))
}

// ---------------------------------------------------------------------------------------------
// Hook

/// Repos-area processing after refs change: ref activity log, ruleset rule suites, Pages builds.
pub async fn on_refs_changed(s: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) {
    if let Err(e) = record_activity(s, ctx, actor, changed).await {
        tracing::warn!(error = %format!("{e:#}"), "repo activity recording failed");
    }
    if let Err(e) = super::repos_rules::record_rule_suites(s, ctx, actor, changed).await {
        tracing::warn!(error = %format!("{e:#}"), "rule suite recording failed");
    }
    super::repos_pages::on_refs_changed(s, ctx, actor, changed).await;
}

/// Whether the update of `refname` to `new` is a pull request merge (merge commit, squash /
/// rebase result recorded on the PR, or a fast-forward to the PR head).
pub async fn is_pr_merge(s: &AppState, ctx: &RepoCtx, refname: &str, new: &str) -> bool {
    let Some(branch) = refname.strip_prefix("refs/heads/") else { return false };
    let parents: Vec<String> = super::plat::git_read(s, ctx.repo.id, &["rev-list", "--parents", "-n", "1", new])
        .await
        .ok()
        .flatten()
        .map(|o| String::from_utf8_lossy(&o).split_whitespace().skip(1).map(str::to_string).collect())
        .unwrap_or_default();
    let n: Option<i64> = sqlx::query_scalar(
        "SELECT 1::bigint FROM pull_requests WHERE repo_id = $1 AND base_ref = $2
            AND (merge_commit_sha = $3 OR head_sha = $3 OR (cardinality($4::text[]) > 1 AND head_sha = ANY($4))) LIMIT 1",
    )
    .bind(ctx.repo.id)
    .bind(branch)
    .bind(new)
    .bind(&parents)
    .fetch_optional(&s.db)
    .await
    .unwrap_or(None);
    n.is_some()
}

async fn record_activity(s: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) -> anyhow::Result<()> {
    for (refname, old, new) in changed {
        if !refname.starts_with("refs/heads/") {
            continue;
        }
        let kind = if old == ZERO_SHA {
            "branch_creation"
        } else if new == ZERO_SHA {
            "branch_deletion"
        } else if is_pr_merge(s, ctx, refname, new).await {
            "pr_merge"
        } else {
            let path = s.git.sync(ctx.repo.id).await?;
            if crate::git::read::is_ancestor(&s.git.git, &path, old, new).await.unwrap_or(true) { "push" } else { "force_push" }
        };
        sqlx::query("INSERT INTO repo_ref_activity (repo_id, actor_id, ref, before, after, activity_type) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(ctx.repo.id)
            .bind(actor.map(|a| a.id))
            .bind(refname)
            .bind(old)
            .bind(new)
            .bind(kind)
            .execute(&s.db)
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Activity endpoint

#[derive(Deserialize)]
struct ActivityQuery {
    direction: Option<String>,
    per_page: Option<i64>,
    before: Option<String>,
    after: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    actor: Option<String>,
    time_period: Option<String>,
    activity_type: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ActivityRow {
    id: i64,
    actor_id: Option<i64>,
    #[sqlx(rename = "ref")]
    git_ref: String,
    before: String,
    after: String,
    activity_type: String,
    created_at: DateTime<Utc>,
}

fn simple_invalid(msg: &str) -> ApiError {
    let mut e = ApiError::unprocessable("Validation Failed");
    e.errors = Some(json!([msg]));
    e
}

async fn activity(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>, Query(q): Query<ActivityQuery>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let asc = match q.direction.as_deref().unwrap_or("desc") {
        "asc" => true,
        "desc" => false,
        _ => return Err(simple_invalid("direction must be asc or desc")),
    };
    let per = q.per_page.unwrap_or(30).clamp(1, 100);
    let interval = match q.time_period.as_deref() {
        None => None,
        Some("day") => Some("1 day"),
        Some("week") => Some("7 days"),
        Some("month") => Some("1 month"),
        Some("quarter") => Some("3 months"),
        Some("year") => Some("1 year"),
        Some(_) => return Err(simple_invalid("time_period must be one of day, week, month, quarter, year")),
    };
    const TYPES: [&str; 6] = ["push", "force_push", "branch_deletion", "branch_creation", "pr_merge", "merge_queue_merge"];
    if let Some(t) = q.activity_type.as_deref() {
        if !TYPES.contains(&t) {
            return Err(simple_invalid("invalid activity_type"));
        }
    }
    let parse_cursor = |c: &Option<String>| -> ApiResult<Option<i64>> {
        c.as_deref().map(|x| x.parse::<i64>().map_err(|_| simple_invalid("invalid cursor"))).transpose()
    };
    let after = parse_cursor(&q.after)?;
    let before = parse_cursor(&q.before)?;
    let git_ref = q.git_ref.map(|r| if r.starts_with("refs/") { r } else { format!("refs/heads/{r}") });
    let actor_id = match q.actor.as_deref() {
        Some(a) => Some(db::user_by_login(&s.db, a).await.map(|x| x.id).unwrap_or(-1)),
        None => None,
    };
    let (cmp_after, cmp_before, order) = if asc { (">", "<", "ASC") } else { ("<", ">", "DESC") };
    let time = interval.map(|i| format!(" AND created_at > now() - interval '{i}'")).unwrap_or_default();
    // `before` walks backwards: fetch in reverse order, then flip.
    let (order_sql, flip) = if before.is_some() && after.is_none() { (if asc { "DESC" } else { "ASC" }, true) } else { (order, false) };
    let sql = format!(
        "SELECT id, actor_id, ref, before, after, activity_type, created_at FROM repo_ref_activity
          WHERE repo_id = $1 AND ($2::text IS NULL OR ref = $2) AND ($3::bigint IS NULL OR actor_id = $3)
            AND ($4::text IS NULL OR activity_type = $4)
            AND ($5::bigint IS NULL OR id {cmp_after} $5) AND ($6::bigint IS NULL OR id {cmp_before} $6){time}
          ORDER BY id {order_sql} LIMIT $7"
    );
    let mut rows: Vec<ActivityRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ctx.repo.id)
        .bind(&git_ref)
        .bind(actor_id)
        .bind(&q.activity_type)
        .bind(after)
        .bind(before)
        .bind(per + 1)
        .fetch_all(&s.db)
        .await?;
    let more = rows.len() as i64 > per;
    rows.truncate(per as usize);
    if flip {
        rows.reverse();
    }
    let ids: Vec<i64> = rows.iter().filter_map(|r| r.actor_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let api = s.api_url();
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id, "node_id": crate::ids::encode("RepositoryActivity", r.id),
                "before": r.before, "after": r.after, "ref": r.git_ref,
                "timestamp": crate::json::ts(r.created_at), "activity_type": r.activity_type,
                "actor": r.actor_id.and_then(|i| users.get(&i)).map(|u| crate::json::user::simple(&api, s.base_url(), u)).unwrap_or(Value::Null),
            })
        })
        .collect();
    let mut links = Vec::new();
    let base_q: Vec<(String, String)> = uri
        .0
        .query()
        .map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().filter(|(k, _)| k != "after" && k != "before").collect())
        .unwrap_or_default();
    let mk = |k: &str, v: i64| {
        let mut pairs = base_q.clone();
        pairs.push((k.to_string(), v.to_string()));
        let qs = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        format!("{}{}?{qs}", s.base_url(), uri.0.path())
    };
    if let (Some(last), true) = (rows.last(), more || flip) {
        if more || before.is_some() {
            links.push(format!("<{}>; rel=\"next\"", mk("after", last.id)));
        }
    }
    if let Some(first) = rows.first() {
        if after.is_some() || (flip && more) {
            links.push(format!("<{}>; rel=\"prev\"", mk("before", first.id)));
        }
    }
    let mut res = axum::Json(Value::Array(items)).into_response();
    if !links.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&links.join(", ")) {
            res.headers_mut().insert(header::LINK, v);
        }
    }
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// Statistics

struct Commit {
    time: i64,
    email: String,
    adds: i64,
    dels: i64,
}

/// Commits of the default branch (newest first, at most 10 000), with line counts.
async fn history(s: &AppState, ctx: &RepoCtx) -> ApiResult<Option<Vec<Commit>>> {
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    let head = format!("refs/heads/{}", ctx.repo.default_branch);
    if !refs.contains_key(&head) {
        return Ok(None);
    }
    let out = super::plat::git_read(s, ctx.repo.id, &["log", "--no-renames", "--numstat", "--format=%x00%at%x09%aE", "-n", "10000", &head, "--"])
        .await?
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&out);
    let mut commits = Vec::new();
    for chunk in text.split('\0').skip(1) {
        let mut lines = chunk.lines();
        let Some(meta) = lines.next() else { continue };
        let (t, email) = meta.split_once('\t').unwrap_or((meta, ""));
        let mut c = Commit { time: t.trim().parse().unwrap_or(0), email: email.trim().to_lowercase(), adds: 0, dels: 0 };
        for l in lines {
            let mut f = l.split('\t');
            if let (Some(a), Some(d)) = (f.next(), f.next()) {
                c.adds += a.parse::<i64>().unwrap_or(0);
                c.dels += d.parse::<i64>().unwrap_or(0);
            }
        }
        commits.push(c);
    }
    Ok(Some(commits))
}

const WEEK: i64 = 7 * 86400;

/// Start (Sunday 00:00 UTC) of the week containing `t`.
fn week_of(t: i64) -> i64 {
    // 1970-01-01 was a Thursday; Sunday 1970-01-04 is day 3.
    let days = t.div_euclid(86400);
    let since_sunday = (days - 3).rem_euclid(7);
    (days - since_sunday) * 86400
}

fn empty_204() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

async fn load(s: &AppState, u: Option<&User>, o: &str, r: &str) -> ApiResult<(RepoCtx, Option<Vec<Commit>>)> {
    let ctx = db::load_repo(&s.db, u, o, r).await?;
    let h = history(s, &ctx).await?;
    Ok((ctx, h.filter(|c| !c.is_empty())))
}

async fn code_frequency(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let (_, Some(commits)) = load(&s, u.as_ref(), &o, &r).await? else { return Ok(empty_204()) };
    let mut weeks: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
    for c in &commits {
        let e = weeks.entry(week_of(c.time)).or_default();
        e.0 += c.adds;
        e.1 += c.dels;
    }
    let (first, last) = (*weeks.keys().next().unwrap(), *weeks.keys().last().unwrap());
    let mut out = Vec::new();
    let mut w = first;
    while w <= last {
        let (a, d) = weeks.get(&w).copied().unwrap_or((0, 0));
        out.push(json!([w, a, -d]));
        w += WEEK;
    }
    Ok(axum::Json(Value::Array(out)).into_response())
}

fn last_52_weeks() -> Vec<i64> {
    let this = week_of(Utc::now().timestamp());
    (0..52).rev().map(|i| this - i * WEEK).collect()
}

async fn commit_activity(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let (_, Some(commits)) = load(&s, u.as_ref(), &o, &r).await? else { return Ok(empty_204()) };
    let weeks = last_52_weeks();
    let mut days: HashMap<i64, [i64; 7]> = HashMap::new();
    for c in &commits {
        let w = week_of(c.time);
        let d = ((c.time - w) / 86400).clamp(0, 6) as usize;
        days.entry(w).or_default()[d] += 1;
    }
    let out: Vec<Value> = weeks
        .iter()
        .map(|w| {
            let d = days.get(w).copied().unwrap_or_default();
            json!({"days": d, "total": d.iter().sum::<i64>(), "week": w})
        })
        .collect();
    Ok(axum::Json(Value::Array(out)).into_response())
}

async fn contributors(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let (_, Some(commits)) = load(&s, u.as_ref(), &o, &r).await? else { return Ok(empty_204()) };
    let emails: Vec<String> = commits.iter().map(|c| c.email.clone()).collect();
    let users = super::plat::users_by_emails(&s.db, &emails).await?;
    let first = commits.iter().map(|c| week_of(c.time)).min().unwrap_or(0);
    let last = commits.iter().map(|c| week_of(c.time)).max().unwrap_or(0);
    let mut per: HashMap<i64, (User, BTreeMap<i64, (i64, i64, i64)>, i64)> = HashMap::new();
    for c in &commits {
        let Some(user) = users.get(&c.email) else { continue };
        let e = per.entry(user.id).or_insert_with(|| (user.clone(), BTreeMap::new(), 0));
        let w = e.1.entry(week_of(c.time)).or_default();
        w.0 += c.adds;
        w.1 += c.dels;
        w.2 += 1;
        e.2 += 1;
    }
    let api = s.api_url();
    let mut list: Vec<(i64, Value)> = per
        .into_values()
        .map(|(user, weeks, total)| {
            let mut ws = Vec::new();
            let mut w = first;
            while w <= last {
                let (a, d, c) = weeks.get(&w).copied().unwrap_or((0, 0, 0));
                ws.push(json!({"w": w, "a": a, "d": d, "c": c}));
                w += WEEK;
            }
            (total, json!({"author": crate::json::user::simple(&api, s.base_url(), &user), "total": total, "weeks": ws}))
        })
        .collect();
    list.sort_by_key(|x| x.0);
    Ok(axum::Json(Value::Array(list.into_iter().map(|x| x.1).collect())).into_response())
}

async fn participation(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let (ctx, commits) = load(&s, u.as_ref(), &o, &r).await?;
    let commits = commits.unwrap_or_default();
    let weeks = last_52_weeks();
    let idx: HashMap<i64, usize> = weeks.iter().enumerate().map(|(i, w)| (*w, i)).collect();
    let mut owner_emails: Vec<String> = sqlx::query_scalar("SELECT lower(email) FROM user_emails WHERE user_id = $1")
        .bind(ctx.owner.id)
        .fetch_all(&s.db)
        .await?;
    if let Some(e) = &ctx.owner.email {
        owner_emails.push(e.to_lowercase());
    }
    let (mut all, mut owner) = (vec![0i64; 52], vec![0i64; 52]);
    for c in &commits {
        if let Some(&i) = idx.get(&week_of(c.time)) {
            all[i] += 1;
            if owner_emails.contains(&c.email) {
                owner[i] += 1;
            }
        }
    }
    Ok(Json(json!({"all": all, "owner": owner})))
}

async fn punch_card(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let (_, Some(commits)) = load(&s, u.as_ref(), &o, &r).await? else { return Ok(empty_204()) };
    let mut grid = [[0i64; 24]; 7];
    for c in &commits {
        if let Some(t) = DateTime::<Utc>::from_timestamp(c.time, 0) {
            grid[t.weekday().num_days_from_sunday() as usize][t.hour() as usize] += 1;
        }
    }
    let mut out = Vec::new();
    for (d, hours) in grid.iter().enumerate() {
        for (h, n) in hours.iter().enumerate() {
            out.push(json!([d, h, n]));
        }
    }
    Ok(axum::Json(Value::Array(out)).into_response())
}

#[cfg(test)]
mod tests {
    #[test]
    fn weeks_start_on_sunday() {
        // 2024-01-03 (Wednesday) 12:00 UTC -> Sunday 2023-12-31 00:00 UTC.
        assert_eq!(super::week_of(1_704_283_200), 1_703_980_800);
        assert_eq!(super::week_of(1_703_980_800), 1_703_980_800);
    }
}

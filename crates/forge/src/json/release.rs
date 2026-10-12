//! Releases and release assets (`release`, `release-asset` schemas).
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::{opt_str, ts, ts_opt};
use crate::db::{RepoCtx, User};
use crate::ids;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReleaseRow {
    pub id: i64,
    pub repo_id: i64,
    pub tag_name: String,
    pub target_commitish: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub draft: bool,
    pub prerelease: bool,
    pub make_latest: String,
    pub author_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub is_latest: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AssetRow {
    pub id: i64,
    pub release_id: i64,
    pub name: String,
    pub label: Option<String>,
    pub content_type: String,
    pub size: i64,
    pub storage_key: String,
    pub download_count: i64,
    pub uploader_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub state: String,
    pub digest: Option<String>,
}

/// Percent-encodes one URL path segment.
pub fn seg(s: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~')
        .remove(b'+');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

pub fn asset(api: &str, base: &str, ctx: &RepoCtx, tag: &str, a: &AssetRow, uploader: Option<&User>) -> Value {
    let full = ctx.full_name();
    json!({
        "url": format!("{api}/repos/{full}/releases/assets/{}", a.id),
        "browser_download_url": format!("{base}/{full}/releases/download/{}/{}", seg(tag), seg(&a.name)),
        "id": a.id,
        "node_id": ids::encode("ReleaseAsset", a.id),
        "name": a.name,
        "label": opt_str(&a.label),
        "state": a.state,
        "content_type": a.content_type,
        "size": a.size,
        "digest": opt_str(&a.digest),
        "download_count": a.download_count,
        "created_at": ts(a.created_at),
        "updated_at": ts(a.updated_at),
        "uploader": uploader.map(|u| super::user::simple(api, base, u)).unwrap_or(Value::Null),
    })
}

/// `release` schema. `assets` are (asset, uploader) pairs.
pub fn release(
    api: &str,
    base: &str,
    ctx: &RepoCtx,
    r: &ReleaseRow,
    author: Option<&User>,
    assets: &[(&AssetRow, Option<&User>)],
    reactions: Option<&HashMap<String, i64>>,
) -> Value {
    let full = ctx.full_name();
    let url = format!("{api}/repos/{full}/releases/{}", r.id);
    let tag = seg(&r.tag_name);
    let mut v = json!({
        "url": url,
        "html_url": format!("{base}/{full}/releases/tag/{tag}"),
        "assets_url": format!("{url}/assets"),
        "upload_url": format!("{base}/api/uploads/repos/{full}/releases/{}/assets{{?name,label}}", r.id),
        "tarball_url": if r.draft { Value::Null } else { json!(format!("{api}/repos/{full}/tarball/{tag}")) },
        "zipball_url": if r.draft { Value::Null } else { json!(format!("{api}/repos/{full}/zipball/{tag}")) },
        "id": r.id,
        "node_id": ids::encode("Release", r.id),
        "tag_name": r.tag_name,
        "target_commitish": r.target_commitish,
        "name": opt_str(&r.name),
        "body": opt_str(&r.body),
        "draft": r.draft,
        "prerelease": r.prerelease,
        "immutable": false,
        "created_at": ts(r.created_at),
        "published_at": ts_opt(r.published_at),
        "updated_at": ts(r.updated_at),
        "author": super::issue::user_or_ghost(api, base, author),
        "assets": assets.iter().map(|(a, u)| asset(api, base, ctx, &r.tag_name, a, *u)).collect::<Vec<_>>(),
    });
    if let Some(c) = reactions {
        v["reactions"] = super::issue::reactions_json(api, &url, c);
    }
    v
}

//! GitHub JSON shapes for repository content and git objects: commits (`commit`, `git-commit`),
//! refs, trees, blobs, tags, contents, branches, statuses.
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde_json::{json, Value};

use super::ts;
use crate::db::{RepoCtx, User};
use crate::git::read::{CommitInfo, FileChange, Signature, TagInfo, TreeEntry};
use crate::ids;

/// Characters escaped in path segments of generated URLs (`/` is kept).
const PATH: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'[')
    .add(b']')
    .add(b'^')
    .add(b'|')
    .add(b'\\');
/// For query values and single path segments (also escapes `/`, `&`, `=`, `+`).
const COMPONENT: &AsciiSet = &PATH.add(b'/').add(b'&').add(b'=').add(b'+').add(b';');

pub fn enc_path(p: &str) -> String {
    utf8_percent_encode(p, PATH).to_string()
}

pub fn enc_component(p: &str) -> String {
    utf8_percent_encode(p, COMPONENT).to_string()
}

/// URL roots for one repository.
#[derive(Debug, Clone)]
pub struct RepoUrls {
    /// `{base}/api/v3`
    pub api: String,
    /// `{base}`
    pub base: String,
    /// `{base}/api/v3/repos/{owner}/{repo}`
    pub repo_api: String,
    /// `{base}/{owner}/{repo}`
    pub html: String,
    /// Repository id (for batch lookups such as commit comment counts).
    pub repo_id: i64,
}

impl RepoUrls {
    pub fn new(api: &str, base: &str, ctx: &RepoCtx) -> RepoUrls {
        let full = ctx.full_name();
        RepoUrls {
            api: api.to_string(),
            base: base.to_string(),
            repo_api: format!("{api}/repos/{full}"),
            html: format!("{base}/{full}"),
            repo_id: ctx.repo.id,
        }
    }
}

pub fn git_user(s: &Signature) -> Value {
    json!({ "name": s.name, "email": s.email, "date": ts(s.date()) })
}

pub fn verification(c: &CommitInfo) -> Value {
    match &c.signature {
        Some(sig) => json!({
            "verified": false, "reason": "unknown_key", "signature": sig, "payload": c.payload, "verified_at": null,
        }),
        None => json!({ "verified": false, "reason": "unsigned", "signature": null, "payload": null, "verified_at": null }),
    }
}

/// `git-commit` schema (GET/POST /git/commits).
pub fn git_commit(u: &RepoUrls, c: &CommitInfo) -> Value {
    json!({
        "sha": c.sha,
        "node_id": ids::encode_str("Commit", &c.sha),
        "url": format!("{}/git/commits/{}", u.repo_api, c.sha),
        "html_url": format!("{}/commit/{}", u.html, c.sha),
        "author": git_user(&c.author),
        "committer": git_user(&c.committer),
        "message": c.message.trim_end_matches('\n'),
        "tree": { "sha": c.tree, "url": format!("{}/git/trees/{}", u.repo_api, c.tree) },
        "parents": c.parents.iter().map(|p| json!({
            "sha": p,
            "url": format!("{}/git/commits/{p}", u.repo_api),
            "html_url": format!("{}/commit/{p}", u.html),
        })).collect::<Vec<_>>(),
        "verification": verification(c),
    })
}

/// `commit` schema (GET /commits, /commits/{ref}, compare). `author`/`committer` are the forge
/// users matched by email. `stats`/`files` are added by the caller for single commits.
pub fn commit(u: &RepoUrls, c: &CommitInfo, author: Option<&User>, committer: Option<&User>, comment_count: i64) -> Value {
    let user = |x: Option<&User>| x.map(|x| super::user::simple(&u.api, &u.base, x)).unwrap_or(Value::Null);
    json!({
        "url": format!("{}/commits/{}", u.repo_api, c.sha),
        "sha": c.sha,
        "node_id": ids::encode_str("Commit", &c.sha),
        "html_url": format!("{}/commit/{}", u.html, c.sha),
        "comments_url": format!("{}/commits/{}/comments", u.repo_api, c.sha),
        "commit": {
            "url": format!("{}/git/commits/{}", u.repo_api, c.sha),
            "author": git_user(&c.author),
            "committer": git_user(&c.committer),
            "message": c.message.trim_end_matches('\n'),
            "comment_count": comment_count,
            "tree": { "sha": c.tree, "url": format!("{}/git/trees/{}", u.repo_api, c.tree) },
            "verification": verification(c),
        },
        "author": user(author),
        "committer": user(committer),
        "parents": c.parents.iter().map(|p| json!({
            "sha": p,
            "url": format!("{}/commits/{p}", u.repo_api),
            "html_url": format!("{}/commit/{p}", u.html),
        })).collect::<Vec<_>>(),
    })
}

/// `diff-entry` schema. `rev` is the commit the file URLs point at.
pub fn diff_entry(u: &RepoUrls, rev: &str, f: &FileChange) -> Value {
    let removed = f.status == "removed";
    let sha = if removed { &f.old_sha } else { &f.new_sha };
    let path = enc_path(&f.path);
    let mut v = json!({
        "sha": sha,
        "filename": f.path,
        "status": f.status,
        "additions": f.additions,
        "deletions": f.deletions,
        "changes": f.additions + f.deletions,
        "blob_url": format!("{}/blob/{rev}/{path}", u.html),
        "raw_url": format!("{}/raw/{rev}/{path}", u.html),
        "contents_url": format!("{}/contents/{path}?ref={rev}", u.repo_api),
    });
    if let Some(p) = &f.patch {
        v["patch"] = json!(p);
    }
    if let Some(prev) = &f.previous_path {
        v["previous_filename"] = json!(prev);
    }
    v
}

pub fn stats(files: &[FileChange]) -> Value {
    let a: u64 = files.iter().map(|f| f.additions).sum();
    let d: u64 = files.iter().map(|f| f.deletions).sum();
    json!({ "additions": a, "deletions": d, "total": a + d })
}

/// `git-ref` schema. `kind` is the target object type (commit, tag, ...).
pub fn git_ref(u: &RepoUrls, name: &str, sha: &str, kind: &str) -> Value {
    let obj_url = match kind {
        "tag" => format!("{}/git/tags/{sha}", u.repo_api),
        "tree" => format!("{}/git/trees/{sha}", u.repo_api),
        "blob" => format!("{}/git/blobs/{sha}", u.repo_api),
        _ => format!("{}/git/commits/{sha}", u.repo_api),
    };
    json!({
        "ref": name,
        "node_id": ids::encode_str("Ref", name),
        "url": format!("{}/git/{}", u.repo_api, enc_path(name)),
        "object": { "type": kind, "sha": sha, "url": obj_url },
    })
}

/// One entry of a `git-tree`.
pub fn tree_entry(u: &RepoUrls, e: &TreeEntry) -> Value {
    let mut v = json!({ "path": e.path, "mode": e.mode, "type": e.kind, "sha": e.sha });
    match e.kind.as_str() {
        "blob" => {
            v["size"] = json!(e.size.unwrap_or(0));
            v["url"] = json!(format!("{}/git/blobs/{}", u.repo_api, e.sha));
        }
        "tree" => v["url"] = json!(format!("{}/git/trees/{}", u.repo_api, e.sha)),
        _ => {}
    }
    v
}

pub fn git_tree(u: &RepoUrls, sha: &str, entries: &[TreeEntry], truncated: bool) -> Value {
    json!({
        "sha": sha,
        "url": format!("{}/git/trees/{sha}", u.repo_api),
        "tree": entries.iter().map(|e| tree_entry(u, e)).collect::<Vec<_>>(),
        "truncated": truncated,
    })
}

/// base64 the way GitHub returns it: wrapped at 60 characters with `\n`.
pub fn base64_wrapped(data: &[u8]) -> String {
    use base64::Engine;
    let s = base64::engine::general_purpose::STANDARD.encode(data);
    let mut out = String::with_capacity(s.len() + s.len() / 60 + 1);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && i % 60 == 0 {
            out.push('\n');
        }
        out.push(ch);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

pub fn blob(u: &RepoUrls, sha: &str, data: &[u8]) -> Value {
    json!({
        "sha": sha,
        "node_id": ids::encode_str("Blob", sha),
        "size": data.len(),
        "url": format!("{}/git/blobs/{sha}", u.repo_api),
        "content": base64_wrapped(data),
        "encoding": "base64",
    })
}

/// `git-tag` schema.
pub fn git_tag(u: &RepoUrls, t: &TagInfo) -> Value {
    let obj_url = match t.kind.as_str() {
        "tag" => format!("{}/git/tags/{}", u.repo_api, t.object),
        "tree" => format!("{}/git/trees/{}", u.repo_api, t.object),
        "blob" => format!("{}/git/blobs/{}", u.repo_api, t.object),
        _ => format!("{}/git/commits/{}", u.repo_api, t.object),
    };
    let tagger = t
        .tagger
        .as_ref()
        .map(git_user)
        .unwrap_or_else(|| json!({"name": "", "email": "", "date": "1970-01-01T00:00:00Z"}));
    let verification = match &t.signature {
        Some(sig) => json!({"verified": false, "reason": "unknown_key", "signature": sig, "payload": null, "verified_at": null}),
        None => json!({"verified": false, "reason": "unsigned", "signature": null, "payload": null, "verified_at": null}),
    };
    json!({
        "node_id": ids::encode_str("Tag", &t.sha),
        "tag": t.tag,
        "sha": t.sha,
        "url": format!("{}/git/tags/{}", u.repo_api, t.sha),
        "message": t.message,
        "tagger": tagger,
        "object": { "sha": t.object, "type": t.kind, "url": obj_url },
        "verification": verification,
    })
}

/// Content type reported by the contents API for a tree entry.
pub fn content_type(e: &TreeEntry, in_listing: bool) -> &'static str {
    match (e.mode.as_str(), e.kind.as_str()) {
        (_, "tree") => "dir",
        ("120000", _) => "symlink",
        // GitHub lists submodules as "file" in directory listings for backwards compatibility.
        ("160000", _) if in_listing => "file",
        ("160000", _) => "submodule",
        _ => "file",
    }
}

/// Common fields of a contents API item (`content-directory` entry / `content-tree`).
pub fn content_item(u: &RepoUrls, rev: &str, e: &TreeEntry, in_listing: bool) -> Value {
    let kind = content_type(e, in_listing);
    let path = enc_path(&e.path);
    let refq = enc_component(rev);
    let url = format!("{}/contents/{path}?ref={refq}", u.repo_api);
    let submodule = e.mode == "160000";
    let (git_url, html_url, download_url) = if submodule {
        (Value::Null, Value::Null, Value::Null)
    } else if kind == "dir" {
        (
            json!(format!("{}/git/trees/{}", u.repo_api, e.sha)),
            json!(format!("{}/tree/{}/{path}", u.html, enc_path(rev))),
            Value::Null,
        )
    } else {
        (
            json!(format!("{}/git/blobs/{}", u.repo_api, e.sha)),
            json!(format!("{}/blob/{}/{path}", u.html, enc_path(rev))),
            json!(format!("{}/raw/{}/{path}", u.html, enc_path(rev))),
        )
    };
    json!({
        "type": kind,
        "size": if kind == "dir" || submodule { 0 } else { e.size.unwrap_or(0) },
        "name": e.name(),
        "path": e.path,
        "sha": e.sha,
        "url": url,
        "git_url": git_url,
        "html_url": html_url,
        "download_url": download_url,
        "_links": { "self": url, "git": git_url, "html": html_url },
    })
}

/// `short-branch` / `branch-short`.
pub fn short_branch(u: &RepoUrls, name: &str, sha: &str, protected: bool) -> Value {
    json!({
        "name": name,
        "commit": { "sha": sha, "url": format!("{}/commits/{sha}", u.repo_api) },
        "protected": protected,
    })
}

/// `tag` schema (GET /tags).
pub fn tag(u: &RepoUrls, name: &str, commit_sha: &str) -> Value {
    let n = enc_path(name);
    json!({
        "name": name,
        "commit": { "sha": commit_sha, "url": format!("{}/commits/{commit_sha}", u.repo_api) },
        "zipball_url": format!("{}/zipball/refs/tags/{n}", u.repo_api),
        "tarball_url": format!("{}/tarball/refs/tags/{n}", u.repo_api),
        "node_id": ids::encode_str("Ref", &format!("refs/tags/{name}")),
    })
}

/// A commit status row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StatusRow {
    pub id: i64,
    pub sha: String,
    pub state: String,
    pub context: String,
    pub description: Option<String>,
    pub target_url: Option<String>,
    pub creator_id: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// `status` schema (with `creator`); `simple` drops `creator` for `simple-commit-status`.
pub fn status(u: &RepoUrls, s: &StatusRow, creator: Option<&User>, simple: bool) -> Value {
    let mut v = json!({
        "url": format!("{}/statuses/{}", u.repo_api, s.sha),
        "avatar_url": creator.map(|c| format!("{}/avatars/{}", u.base, c.login)),
        "id": s.id,
        "node_id": ids::encode("StatusContext", s.id),
        "state": s.state,
        "description": s.description,
        "target_url": s.target_url,
        "context": s.context,
        "created_at": ts(s.created_at),
        "updated_at": ts(s.updated_at),
    });
    if !simple {
        v["creator"] = creator.map(|c| super::user::simple(&u.api, &u.base, c)).unwrap_or(Value::Null);
    }
    v
}

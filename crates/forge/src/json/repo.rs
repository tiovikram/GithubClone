use serde_json::{json, Value};

use super::{opt_str, ts, ts_opt};
use crate::db::{Perm, RepoCtx};
use crate::ids;

/// `minimal-repository`/`repository` shaped object. `full` adds the `full-repository` extras
/// (and `parent`/`source` when given).
pub fn repo(api: &str, base: &str, c: &RepoCtx) -> Value {
    let r = &c.repo;
    let full_name = c.full_name();
    let url = format!("{api}/repos/{full_name}");
    let perm = c.perm;
    json!({
        "id": r.id,
        "node_id": ids::encode("Repository", r.id),
        "name": r.name,
        "full_name": full_name,
        "private": r.private,
        "owner": super::user::simple(api, base, &c.owner),
        "html_url": format!("{base}/{full_name}"),
        "description": opt_str(&r.description),
        "fork": r.fork_parent_id.is_some(),
        "url": url,
        "forks_url": format!("{url}/forks"),
        "keys_url": format!("{url}/keys{{/key_id}}"),
        "collaborators_url": format!("{url}/collaborators{{/collaborator}}"),
        "teams_url": format!("{url}/teams"),
        "hooks_url": format!("{url}/hooks"),
        "issue_events_url": format!("{url}/issues/events{{/number}}"),
        "events_url": format!("{url}/events"),
        "assignees_url": format!("{url}/assignees{{/user}}"),
        "branches_url": format!("{url}/branches{{/branch}}"),
        "tags_url": format!("{url}/tags"),
        "blobs_url": format!("{url}/git/blobs{{/sha}}"),
        "git_tags_url": format!("{url}/git/tags{{/sha}}"),
        "git_refs_url": format!("{url}/git/refs{{/sha}}"),
        "trees_url": format!("{url}/git/trees{{/sha}}"),
        "statuses_url": format!("{url}/statuses/{{sha}}"),
        "languages_url": format!("{url}/languages"),
        "stargazers_url": format!("{url}/stargazers"),
        "contributors_url": format!("{url}/contributors"),
        "subscribers_url": format!("{url}/subscribers"),
        "subscription_url": format!("{url}/subscription"),
        "commits_url": format!("{url}/commits{{/sha}}"),
        "git_commits_url": format!("{url}/git/commits{{/sha}}"),
        "comments_url": format!("{url}/comments{{/number}}"),
        "issue_comment_url": format!("{url}/issues/comments{{/number}}"),
        "contents_url": format!("{url}/contents/{{+path}}"),
        "compare_url": format!("{url}/compare/{{base}}...{{head}}"),
        "merges_url": format!("{url}/merges"),
        "archive_url": format!("{url}/{{archive_format}}{{/ref}}"),
        "downloads_url": format!("{url}/downloads"),
        "issues_url": format!("{url}/issues{{/number}}"),
        "pulls_url": format!("{url}/pulls{{/number}}"),
        "milestones_url": format!("{url}/milestones{{/number}}"),
        "notifications_url": format!("{url}/notifications{{?since,all,participating}}"),
        "labels_url": format!("{url}/labels{{/name}}"),
        "releases_url": format!("{url}/releases{{/id}}"),
        "deployments_url": format!("{url}/deployments"),
        "created_at": ts(r.created_at),
        "updated_at": ts(r.updated_at),
        "pushed_at": ts_opt(r.pushed_at),
        "git_url": format!("git://{}/{full_name}.git", host_of(base)),
        "ssh_url": crate::ssh::ssh_url(base, &full_name),
        "clone_url": format!("{base}/{full_name}.git"),
        "svn_url": format!("{base}/{full_name}"),
        "homepage": opt_str(&r.homepage),
        "size": r.size_kb,
        "stargazers_count": r.stargazers_count,
        "watchers_count": r.stargazers_count,
        "language": opt_str(&r.language),
        "has_issues": r.has_issues,
        "has_projects": r.has_projects,
        "has_downloads": true,
        "has_wiki": r.has_wiki,
        "has_pages": false,
        "has_discussions": r.has_discussions,
        "forks_count": r.forks_count,
        "mirror_url": null,
        "archived": r.archived,
        "disabled": false,
        "open_issues_count": r.open_issues_count,
        "license": null,
        "allow_forking": true,
        "is_template": r.is_template,
        "web_commit_signoff_required": false,
        "topics": r.topics,
        "visibility": if r.private { "private" } else { "public" },
        "forks": r.forks_count,
        "open_issues": r.open_issues_count,
        "watchers": r.stargazers_count,
        "default_branch": r.default_branch,
        "permissions": {
            "admin": perm >= Perm::Admin,
            "maintain": perm >= Perm::Maintain,
            "push": perm >= Perm::Write,
            "triage": perm >= Perm::Triage,
            "pull": perm >= Perm::Read,
        },
    })
}

/// `full-repository`: adds merge settings, counts, and optional parent/source.
pub fn full(api: &str, base: &str, c: &RepoCtx, parent: Option<&RepoCtx>, source: Option<&RepoCtx>) -> Value {
    let r = &c.repo;
    let mut v = repo(api, base, c);
    let o = v.as_object_mut().unwrap();
    o.insert("allow_merge_commit".into(), json!(r.allow_merge_commit));
    o.insert("allow_squash_merge".into(), json!(r.allow_squash_merge));
    o.insert("allow_rebase_merge".into(), json!(r.allow_rebase_merge));
    o.insert("allow_auto_merge".into(), json!(r.allow_auto_merge));
    o.insert("delete_branch_on_merge".into(), json!(r.delete_branch_on_merge));
    o.insert("allow_update_branch".into(), json!(false));
    o.insert("use_squash_pr_title_as_default".into(), json!(false));
    o.insert("squash_merge_commit_title".into(), json!("COMMIT_OR_PR_TITLE"));
    o.insert("squash_merge_commit_message".into(), json!("COMMIT_MESSAGES"));
    o.insert("merge_commit_title".into(), json!("MERGE_MESSAGE"));
    o.insert("merge_commit_message".into(), json!("PR_TITLE"));
    o.insert("network_count".into(), json!(r.forks_count));
    o.insert("subscribers_count".into(), json!(0));
    o.insert("temp_clone_token".into(), Value::Null);
    if c.owner.kind == "Organization" {
        o.insert("organization".into(), super::user::simple(api, base, &c.owner));
    }
    if let Some(p) = parent {
        o.insert("parent".into(), repo(api, base, p));
    }
    if let Some(s) = source {
        o.insert("source".into(), repo(api, base, s));
    }
    v
}

pub fn host_of(base: &str) -> String {
    url::Url::parse(base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "localhost".into())
}

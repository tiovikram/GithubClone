//! Repository pull request options that `PATCH /repos/{o}/{r}` accepts and the full repository
//! JSON reports: default merge / squash commit messages, `allow_update_branch`,
//! `use_squash_pr_title_as_default` and `web_commit_signoff_required` (stored in
//! `repo_settings`; `NULL` = GitHub's default). The merge defaults are used by pull request
//! merges that give no `commit_title` / `commit_message`.
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MergeOptsPatch {
    pub merge_commit_title: Option<String>,
    pub merge_commit_message: Option<String>,
    pub squash_merge_commit_title: Option<String>,
    pub squash_merge_commit_message: Option<String>,
    pub allow_update_branch: Option<bool>,
    pub use_squash_pr_title_as_default: Option<bool>,
    pub web_commit_signoff_required: Option<bool>,
}

impl MergeOptsPatch {
    fn is_empty(&self) -> bool {
        self.merge_commit_title.is_none()
            && self.merge_commit_message.is_none()
            && self.squash_merge_commit_title.is_none()
            && self.squash_merge_commit_message.is_none()
            && self.allow_update_branch.is_none()
            && self.use_squash_pr_title_as_default.is_none()
            && self.web_commit_signoff_required.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeOpts {
    pub merge_commit_title: String,
    pub merge_commit_message: String,
    pub squash_merge_commit_title: String,
    pub squash_merge_commit_message: String,
    pub allow_update_branch: bool,
    pub use_squash_pr_title_as_default: bool,
    pub web_commit_signoff_required: bool,
}

impl Default for MergeOpts {
    fn default() -> Self {
        MergeOpts {
            merge_commit_title: "MERGE_MESSAGE".into(),
            merge_commit_message: "PR_TITLE".into(),
            squash_merge_commit_title: "COMMIT_OR_PR_TITLE".into(),
            squash_merge_commit_message: "COMMIT_MESSAGES".into(),
            allow_update_branch: false,
            use_squash_pr_title_as_default: false,
            web_commit_signoff_required: false,
        }
    }
}

type Row = (Option<String>, Option<String>, Option<String>, Option<String>, Option<bool>, Option<bool>, Option<bool>);

pub async fn load(s: &AppState, repo_id: i64) -> ApiResult<MergeOpts> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT merge_commit_title, merge_commit_message, squash_merge_commit_title, squash_merge_commit_message,
                allow_update_branch, use_squash_pr_title_as_default, web_commit_signoff_required
           FROM repo_settings WHERE repo_id = $1",
    )
    .bind(repo_id)
    .fetch_optional(&s.db)
    .await?;
    let d = MergeOpts::default();
    let Some(r) = row else { return Ok(d) };
    Ok(MergeOpts {
        merge_commit_title: r.0.unwrap_or(d.merge_commit_title),
        merge_commit_message: r.1.unwrap_or(d.merge_commit_message),
        squash_merge_commit_title: r.2.unwrap_or(d.squash_merge_commit_title),
        squash_merge_commit_message: r.3.unwrap_or(d.squash_merge_commit_message),
        allow_update_branch: r.4.unwrap_or(false),
        use_squash_pr_title_as_default: r.5.unwrap_or(false),
        web_commit_signoff_required: r.6.unwrap_or(false),
    })
}

fn invalid(field: &str) -> ApiError {
    ApiError::validation("Repository", field, "invalid")
}

/// Applies `p` (validated: GitHub's allowed title/message combinations) and stores the result.
pub async fn save(s: &AppState, repo_id: i64, p: &MergeOptsPatch) -> ApiResult<()> {
    if p.is_empty() {
        return Ok(());
    }
    let mut o = load(s, repo_id).await?;
    if let Some(t) = &p.merge_commit_title {
        o.merge_commit_title = t.clone();
        // Changing only the title picks the matching default message, like github.com.
        if p.merge_commit_message.is_none() {
            o.merge_commit_message = if t == "MERGE_MESSAGE" { "PR_TITLE".into() } else { "PR_BODY".into() };
        }
    }
    if let Some(m) = &p.merge_commit_message {
        o.merge_commit_message = m.clone();
    }
    if let Some(t) = &p.squash_merge_commit_title {
        o.squash_merge_commit_title = t.clone();
        if p.squash_merge_commit_message.is_none() {
            o.squash_merge_commit_message = if t == "COMMIT_OR_PR_TITLE" { "COMMIT_MESSAGES".into() } else { "PR_BODY".into() };
        }
    }
    if let Some(m) = &p.squash_merge_commit_message {
        o.squash_merge_commit_message = m.clone();
    }
    if let Some(b) = p.use_squash_pr_title_as_default {
        o.use_squash_pr_title_as_default = b;
        // The legacy flag maps onto the squash title setting.
        if p.squash_merge_commit_title.is_none() {
            o.squash_merge_commit_title = if b { "PR_TITLE".into() } else { "COMMIT_OR_PR_TITLE".into() };
            if p.squash_merge_commit_message.is_none() {
                o.squash_merge_commit_message = "COMMIT_MESSAGES".into();
            }
        }
    }
    o.use_squash_pr_title_as_default = o.squash_merge_commit_title == "PR_TITLE";
    match (o.merge_commit_title.as_str(), o.merge_commit_message.as_str()) {
        ("MERGE_MESSAGE", "PR_TITLE") | ("PR_TITLE", "PR_BODY" | "BLANK") => {}
        ("MERGE_MESSAGE" | "PR_TITLE", _) => return Err(invalid("merge_commit_message")),
        _ => return Err(invalid("merge_commit_title")),
    }
    match (o.squash_merge_commit_title.as_str(), o.squash_merge_commit_message.as_str()) {
        ("COMMIT_OR_PR_TITLE", "COMMIT_MESSAGES") | ("PR_TITLE", "PR_BODY" | "COMMIT_MESSAGES" | "BLANK") => {}
        ("COMMIT_OR_PR_TITLE" | "PR_TITLE", _) => return Err(invalid("squash_merge_commit_message")),
        _ => return Err(invalid("squash_merge_commit_title")),
    }
    if let Some(b) = p.allow_update_branch {
        o.allow_update_branch = b;
    }
    if let Some(b) = p.web_commit_signoff_required {
        o.web_commit_signoff_required = b;
    }
    sqlx::query(
        "INSERT INTO repo_settings (repo_id, merge_commit_title, merge_commit_message, squash_merge_commit_title, squash_merge_commit_message,
                                    allow_update_branch, use_squash_pr_title_as_default, web_commit_signoff_required)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (repo_id) DO UPDATE SET merge_commit_title = $2, merge_commit_message = $3, squash_merge_commit_title = $4,
             squash_merge_commit_message = $5, allow_update_branch = $6, use_squash_pr_title_as_default = $7,
             web_commit_signoff_required = $8, updated_at = now()",
    )
    .bind(repo_id)
    .bind(&o.merge_commit_title)
    .bind(&o.merge_commit_message)
    .bind(&o.squash_merge_commit_title)
    .bind(&o.squash_merge_commit_message)
    .bind(o.allow_update_branch)
    .bind(o.use_squash_pr_title_as_default)
    .bind(o.web_commit_signoff_required)
    .execute(&s.db)
    .await?;
    Ok(())
}

/// Overwrites the defaults that `json::repo` writes with the stored options.
pub fn apply(v: &mut Value, o: &MergeOpts) {
    for (k, val) in [
        ("merge_commit_title", json!(o.merge_commit_title)),
        ("merge_commit_message", json!(o.merge_commit_message)),
        ("squash_merge_commit_title", json!(o.squash_merge_commit_title)),
        ("squash_merge_commit_message", json!(o.squash_merge_commit_message)),
        ("allow_update_branch", json!(o.allow_update_branch)),
        ("use_squash_pr_title_as_default", json!(o.use_squash_pr_title_as_default)),
        ("web_commit_signoff_required", json!(o.web_commit_signoff_required)),
    ] {
        v[k] = val;
    }
}

/// Default (title, body) of a merge commit.
pub fn merge_message(o: &MergeOpts, number: i64, head_label: &str, pr_title: &str, pr_body: &str) -> (String, String) {
    match o.merge_commit_title.as_str() {
        "PR_TITLE" => (format!("{pr_title} (#{number})"), if o.merge_commit_message == "BLANK" { String::new() } else { pr_body.to_string() }),
        _ => (format!("Merge pull request #{number} from {head_label}"), pr_title.to_string()),
    }
}

/// Default (title, body) of a squash commit; `commits` are the squashed commit messages.
/// (`COMMIT_OR_PR_TITLE` keeps forge's PR-title form for single-commit pulls too.)
pub fn squash_message(o: &MergeOpts, number: i64, pr_title: &str, pr_body: &str, commits: &[String]) -> (String, String) {
    let list = commits.iter().map(|c| format!("* {}", c.trim_end())).collect::<Vec<_>>().join("\n\n");
    let body = match o.squash_merge_commit_message.as_str() {
        "PR_BODY" => pr_body.to_string(),
        "BLANK" => String::new(),
        _ => list,
    };
    (format!("{pr_title} (#{number})"), body)
}

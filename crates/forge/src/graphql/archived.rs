//! Archived repositories are read-only over GraphQL too (the REST side is
//! `security::archived_read_only`).
//!
//! A schema extension looks at every top-level `Mutation` field before it runs: each node id
//! in its arguments (after variable substitution) is mapped to the repository it belongs to.
//! If one of those repositories is archived and visible to the viewer, the field fails with
//! GitHub's error (`type: UNPROCESSABLE`, "Repository was archived so is read-only.") and its
//! value is `null`. Doing it generically means new mutations are covered without extra code.
//! Repositories the viewer can't see are left to the mutation itself, which reports them as
//! unresolvable, so the check never reveals a private repository.
//!
//! The same hook enforces migration locks (REST: `migration::lock_guard`): a mutation naming a
//! repository locked for migration fails with `type: FORBIDDEN`, "Repository has been locked for
//! migration." (only starring and watching, which REST serves outside `/repos/...`, still work).
use std::sync::Arc;

use async_graphql::extensions::{Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest, NextResolve, ResolveInfo};
use async_graphql::{ErrorExtensionValues, Name, Request, ServerError, ServerResult, Value, Variables};

use super::Viewer;
use crate::ids;
use crate::state::AppState;

pub const ARCHIVED_MESSAGE: &str = "Repository was archived so is read-only.";

/// Mutations that may name an archived repository: starring, watching, the archive switch
/// itself and repository settings (REST allows these too), and reading a template.
const ALLOWED: &[&str] = &[
    "addStar",
    "removeStar",
    "updateSubscription",
    "archiveRepository",
    "unarchiveRepository",
    "updateRepository",
    "cloneTemplateRepository",
];

/// Mutations that still work on a repository locked for migration (their REST equivalents live
/// under `/user/...`, which the REST lock guard does not cover).
const LOCK_ALLOWED: &[&str] = &["addStar", "removeStar", "updateSubscription"];

pub const LOCKED_MESSAGE: &str = "Repository has been locked for migration.";

/// Bound on node ids examined per mutation field.
const MAX_IDS: usize = 200;

pub struct ArchivedGuard;

impl ExtensionFactory for ArchivedGuard {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(ArchivedGuardExt)
    }
}

struct ArchivedGuardExt;

/// The request's variables, kept for the resolve hook (field arguments may be variables).
struct RequestVariables(Variables);

#[async_graphql::async_trait::async_trait]
impl Extension for ArchivedGuardExt {
    async fn prepare_request(&self, ctx: &ExtensionContext<'_>, request: Request, next: NextPrepareRequest<'_>) -> ServerResult<Request> {
        let vars = request.variables.clone();
        next.run(ctx, request.data(RequestVariables(vars))).await
    }

    async fn resolve(&self, ctx: &ExtensionContext<'_>, info: ResolveInfo<'_>, next: NextResolve<'_>) -> ServerResult<Option<Value>> {
        let check_archived = !ALLOWED.contains(&info.name);
        let check_locked = !LOCK_ALLOWED.contains(&info.name);
        if info.parent_type == "Mutation" && info.path_node.parent.is_none() && (check_archived || check_locked) {
            if let (Some(state), Some(viewer)) = (ctx.data_opt::<AppState>(), ctx.data_opt::<Viewer>()) {
                let empty = Variables::default();
                let vars = ctx.data_opt::<RequestVariables>().map(|v| &v.0).unwrap_or(&empty);
                let mut node_ids = Vec::new();
                for (_, v) in &info.field.arguments {
                    if let Ok(c) = v.node.clone().into_const_with(|n: Name| vars.get(&n).cloned().ok_or(())) {
                        collect_strings(&c, &mut node_ids);
                    }
                }
                let blocked = match blocked_target(state, &viewer.0, &node_ids).await {
                    Some(Block::Locked) if check_locked => Some(("FORBIDDEN", LOCKED_MESSAGE)),
                    Some(Block::Archived) if check_archived => Some(("UNPROCESSABLE", ARCHIVED_MESSAGE)),
                    _ => None,
                };
                if let Some((kind, msg)) = blocked {
                    let mut ext = ErrorExtensionValues::default();
                    ext.set("type", kind);
                    let mut e = ServerError::new(msg, Some(info.field.name.pos));
                    e.path = vec![async_graphql::PathSegment::Field(info.alias.unwrap_or(info.name).to_string())];
                    e.extensions = Some(ext);
                    return Err(e);
                }
            }
        }
        next.run(ctx, info).await
    }
}

fn collect_strings(v: &Value, out: &mut Vec<String>) {
    if out.len() >= MAX_IDS {
        return;
    }
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::List(l) => l.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// Repository id a node id belongs to (None for nodes that aren't repository content).
pub async fn repo_of_node(db: &sqlx::PgPool, node_id: &str) -> Option<i64> {
    let (kind, key) = ids::decode(node_id)?;
    // Ids whose key starts with the repository id ("<repo>:<rest>").
    if matches!(kind.as_str(), "Ref" | "Commit" | "BranchProtectionRule") {
        return key.split_once(':').and_then(|(r, _)| r.parse().ok());
    }
    let id: i64 = key.parse().ok()?;
    let sql = match kind.as_str() {
        "Repository" => return Some(id),
        "Issue" | "PullRequest" => "SELECT repo_id FROM issues WHERE id = $1",
        "IssueComment" => "SELECT i.repo_id FROM issue_comments c JOIN issues i ON i.id = c.issue_id WHERE c.id = $1",
        "PullRequestReview" => "SELECT i.repo_id FROM pull_reviews r JOIN issues i ON i.id = r.issue_id WHERE r.id = $1",
        "PullRequestReviewComment" => "SELECT i.repo_id FROM review_comments c JOIN issues i ON i.id = c.issue_id WHERE c.id = $1",
        "LinkedBranch" => "SELECT i.repo_id FROM linked_branches b JOIN issues i ON i.id = b.issue_id WHERE b.id = $1",
        "Label" => "SELECT repo_id FROM labels WHERE id = $1",
        "Milestone" => "SELECT repo_id FROM milestones WHERE id = $1",
        "Release" => "SELECT repo_id FROM releases WHERE id = $1",
        "ReleaseAsset" => "SELECT r.repo_id FROM release_assets a JOIN releases r ON r.id = a.release_id WHERE a.id = $1",
        "Discussion" => "SELECT repo_id FROM discussions WHERE id = $1",
        "DiscussionCategory" => "SELECT repo_id FROM discussion_categories WHERE id = $1",
        "DiscussionComment" => "SELECT d.repo_id FROM discussion_comments c JOIN discussions d ON d.id = c.discussion_id WHERE c.id = $1",
        "CommitComment" => "SELECT repo_id FROM commit_comments WHERE id = $1",
        "Deployment" => "SELECT repo_id FROM deployments WHERE id = $1",
        "RepositoryRuleset" => "SELECT repo_id FROM rulesets WHERE id = $1",
        _ => return None,
    };
    sqlx::query_scalar::<_, Option<i64>>(sql).bind(id).fetch_optional(db).await.ok().flatten().flatten()
}

enum Block {
    Locked,
    Archived,
}

/// Why a mutation naming `node_ids` must be refused: one of their repositories (visible to
/// `viewer`) is locked for migration, or archived. Locks win (REST checks them first).
async fn blocked_target(state: &AppState, viewer: &crate::db::User, node_ids: &[String]) -> Option<Block> {
    let mut repos: Vec<i64> = Vec::new();
    for n in node_ids {
        if let Some(r) = repo_of_node(&state.db, n).await {
            if !repos.contains(&r) {
                repos.push(r);
            }
        }
    }
    if repos.is_empty() {
        return None;
    }
    let rows: Vec<(i64, bool, bool)> = sqlx::query_as(
        "SELECT r.id, EXISTS (SELECT 1 FROM migration_locks l WHERE l.repo_id = r.id), r.archived
           FROM repositories r WHERE r.id = ANY($1) AND (r.archived OR EXISTS (SELECT 1 FROM migration_locks l WHERE l.repo_id = r.id))",
    )
    .bind(&repos)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut out = None;
    for (id, locked, archived) in rows {
        if crate::db::load_repo_by_id(&state.db, Some(viewer), id).await.is_err() {
            continue;
        }
        if locked {
            return Some(Block::Locked);
        }
        if archived {
            out = Some(Block::Archived);
        }
    }
    out
}

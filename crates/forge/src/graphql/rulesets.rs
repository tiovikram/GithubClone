//! `Repository.rulesets` / `Organization.rulesets` (`gh ruleset list`), backed by
//! `crate::api::rulesets`.
use std::sync::Arc;

use async_graphql::{Context, Enum, Object, Union, ID};

use super::connection::ConnArgs;
use super::enums::DateTime;
use super::repo::{st, GRepo};
use super::user::GOrg;
use crate::api::rulesets::{self as svc, Level, RulesetRow};
use crate::db::{RepoCtx, User};
use crate::ids;

crate::gql_connection!(RepositoryRulesetConnection, RepositoryRulesetEdge, "RepositoryRulesetConnection", "RepositoryRulesetEdge", GRuleset);
crate::gql_connection!(RepositoryRuleConnection, RepositoryRuleEdge, "RepositoryRuleConnection", "RepositoryRuleEdge", GRule);

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "RepositoryRulesetTarget")]
pub enum RulesetTarget {
    Branch,
    Tag,
    Push,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "RuleEnforcement")]
pub enum RuleEnforcement {
    Disabled,
    Active,
    Evaluate,
}

#[derive(Union, Clone)]
#[graphql(name = "RuleSource")]
pub enum RuleSource {
    Repository(GRepo),
    Organization(GOrg),
}

#[derive(Clone)]
pub struct GRuleset {
    row: Arc<RulesetRow>,
    source: RuleSource,
}

#[derive(Clone)]
pub struct GRule {
    ruleset_id: i64,
    index: usize,
    kind: String,
}

#[Object(name = "RepositoryRuleset")]
impl GRuleset {
    async fn id(&self) -> ID {
        ID(ids::encode("RepositoryRuleset", self.row.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.row.id)
    }
    async fn name(&self) -> &str {
        &self.row.name
    }
    async fn target(&self) -> Option<RulesetTarget> {
        Some(match self.row.target.as_str() {
            "tag" => RulesetTarget::Tag,
            "push" => RulesetTarget::Push,
            _ => RulesetTarget::Branch,
        })
    }
    async fn enforcement(&self) -> RuleEnforcement {
        match self.row.enforcement.as_str() {
            "active" => RuleEnforcement::Active,
            "evaluate" => RuleEnforcement::Evaluate,
            _ => RuleEnforcement::Disabled,
        }
    }
    async fn source(&self) -> RuleSource {
        self.source.clone()
    }
    async fn rules(&self, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> RepositoryRuleConnection {
        let all: Vec<GRule> = self
            .row
            .rules
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, r)| GRule { ruleset_id: self.row.id, index, kind: r.get("type").and_then(|t| t.as_str()).unwrap_or_default().to_string() })
            .collect();
        RepositoryRuleConnection::from_all(all, &ConnArgs::new(first, after, last, before))
    }
    async fn created_at(&self) -> DateTime {
        super::enums::dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        super::enums::dt(self.row.updated_at)
    }
}

#[Object(name = "RepositoryRule")]
impl GRule {
    async fn id(&self) -> ID {
        ID(ids::encode_str("RepositoryRule", &format!("{}:{}", self.ruleset_id, self.index)))
    }
    /// GitHub's `RepositoryRuleType` enum value (e.g. `DELETION`), as a string.
    #[graphql(name = "type")]
    async fn kind(&self) -> String {
        self.kind.to_uppercase()
    }
}

fn wrap(rows: Vec<RulesetRow>, level: &Level, repo: Option<Arc<RepoCtx>>, org: impl Fn() -> Arc<User>) -> Vec<GRuleset> {
    rows.into_iter()
        .map(|r| {
            let source = match (&repo, r.org_id) {
                (Some(rc), None) => RuleSource::Repository(GRepo::new(rc.clone())),
                _ => RuleSource::Organization(GOrg(org())),
            };
            let _ = level;
            GRuleset { row: Arc::new(r), source }
        })
        .collect()
}

/// `Repository.rulesets(includeParents:)`.
pub async fn for_repo(
    ctx: &Context<'_>,
    repo: Arc<RepoCtx>,
    include_parents: bool,
    args: ConnArgs,
) -> async_graphql::Result<RepositoryRulesetConnection> {
    let level = Level::Repo((*repo).clone());
    let rows = svc::list_rulesets(st(ctx), &level, include_parents, None).await.map_err(super::api_err)?;
    let owner = Arc::new(repo.owner.clone());
    Ok(RepositoryRulesetConnection::from_all(wrap(rows, &level, Some(repo), || owner.clone()), &args))
}

/// `Organization.rulesets`.
pub async fn for_org(ctx: &Context<'_>, org: Arc<User>, args: ConnArgs) -> async_graphql::Result<RepositoryRulesetConnection> {
    let level = Level::Org((*org).clone());
    let rows = svc::list_rulesets(st(ctx), &level, true, None).await.map_err(super::api_err)?;
    Ok(RepositoryRulesetConnection::from_all(wrap(rows, &level, None, || org.clone()), &args))
}

//! `User.gists` and the `Gist` / `GistFile` types (`gh gist list`), backed by the gist tables of
//! `crate::api::gists`.
use std::sync::Arc;

use async_graphql::{Context, Enum, InputObject, Object, ID};

use super::connection::ConnArgs;
use super::enums::{dt, DateTime, OrderDirection, Uri};
use super::repo::{st, viewer};
use super::user::RepositoryOwner;
use crate::api::gists::GistRow;
use crate::db::User;

crate::gql_connection!(GistConnection, GistEdge, "GistConnection", "GistEdge", GGist);

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum GistPrivacy {
    Public,
    Secret,
    All,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum GistOrderField {
    CreatedAt,
    UpdatedAt,
    PushedAt,
}

#[derive(InputObject, Clone, Copy, Debug)]
pub struct GistOrder {
    pub field: GistOrderField,
    pub direction: OrderDirection,
}

#[derive(Clone)]
pub struct GGist {
    row: Arc<GistRow>,
    owner: Arc<User>,
}

#[derive(Clone)]
pub struct GGistFile {
    name: String,
    content: String,
}

/// `User.gists`: the owner sees secret gists too; everyone else only public ones.
pub async fn for_user(
    ctx: &Context<'_>,
    owner: Arc<User>,
    privacy: Option<GistPrivacy>,
    order_by: Option<GistOrder>,
    args: ConnArgs,
) -> async_graphql::Result<GistConnection> {
    let is_owner = viewer(ctx).is_some_and(|v| v.id == owner.id);
    let privacy = match privacy.unwrap_or(GistPrivacy::Public) {
        _ if !is_owner => GistPrivacy::Public,
        p => p,
    };
    let filter = match privacy {
        GistPrivacy::Public => "AND public",
        GistPrivacy::Secret => "AND NOT public",
        GistPrivacy::All => "",
    };
    let order = order_by.unwrap_or(GistOrder { field: GistOrderField::PushedAt, direction: OrderDirection::Desc });
    let col = match order.field {
        GistOrderField::CreatedAt => "created_at",
        _ => "updated_at",
    };
    let sql = format!("SELECT * FROM gists WHERE owner_id = $1 {filter} ORDER BY {col} {}, id {}", order.direction.sql(), order.direction.sql());
    // Only fixed fragments are interpolated.
    let rows: Vec<GistRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(owner.id).fetch_all(&st(ctx).db).await?;
    let all = rows.into_iter().map(|r| GGist { row: Arc::new(r), owner: owner.clone() }).collect();
    Ok(GistConnection::from_all(all, &args))
}

#[Object(name = "Gist")]
impl GGist {
    async fn id(&self) -> ID {
        ID(crate::ids::encode("Gist", self.row.id))
    }
    async fn name(&self) -> &str {
        &self.row.gist_id
    }
    async fn description(&self) -> Option<&str> {
        self.row.description.as_deref()
    }
    async fn is_public(&self) -> bool {
        self.row.public
    }
    async fn is_fork(&self) -> bool {
        self.row.fork_of.is_some()
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn pushed_at(&self) -> Option<DateTime> {
        Some(dt(self.row.updated_at))
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/gist/{}", st(ctx).base_url(), self.row.gist_id))
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/gist/{}", self.row.gist_id))
    }
    async fn owner(&self) -> Option<RepositoryOwner> {
        Some(RepositoryOwner::from_user(self.owner.clone()))
    }
    async fn stargazer_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM gist_stars WHERE gist_id = $1").bind(self.row.id).fetch_one(&st(ctx).db).await?;
        Ok(n as i32)
    }
    async fn files(&self, ctx: &Context<'_>, #[graphql(default = 10)] limit: i32) -> async_graphql::Result<Option<Vec<Option<GGistFile>>>> {
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT filename, content FROM gist_files WHERE gist_id = $1 ORDER BY filename LIMIT $2")
            .bind(self.row.id)
            .bind(limit.clamp(1, 300) as i64)
            .fetch_all(&st(ctx).db)
            .await?;
        Ok(Some(rows.into_iter().map(|(name, content)| Some(GGistFile { name, content })).collect()))
    }
}

#[Object(name = "GistFile")]
impl GGistFile {
    async fn name(&self) -> Option<&str> {
        Some(&self.name)
    }
    async fn encoded_name(&self) -> Option<String> {
        Some(url::form_urlencoded::byte_serialize(self.name.as_bytes()).collect())
    }
    async fn encoding(&self) -> Option<&str> {
        Some("UTF-8")
    }
    async fn extension(&self) -> Option<String> {
        self.name.rsplit_once('.').map(|(_, e)| format!(".{e}"))
    }
    async fn size(&self) -> Option<i32> {
        Some(self.content.len() as i32)
    }
    async fn is_image(&self) -> bool {
        false
    }
    async fn is_truncated(&self) -> bool {
        false
    }
    async fn text(&self, truncate: Option<i32>) -> Option<String> {
        Some(match truncate {
            Some(n) if n >= 0 => self.content.chars().take(n as usize).collect(),
            _ => self.content.clone(),
        })
    }
}

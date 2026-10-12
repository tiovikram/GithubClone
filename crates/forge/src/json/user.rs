use serde_json::{json, Value};

use super::{opt_str, ts};
use crate::db::User;
use crate::ids;

fn node_kind(u: &User) -> &str {
    u.kind.as_str()
}

/// `simple-user` schema.
pub fn simple(api: &str, base: &str, u: &User) -> Value {
    let url = format!("{api}/users/{}", u.login);
    json!({
        "login": u.login,
        "id": u.id,
        "node_id": ids::encode(node_kind(u), u.id),
        "avatar_url": format!("{base}/avatars/{}", u.login),
        "gravatar_id": "",
        "url": url,
        "html_url": format!("{base}/{}", u.login),
        "followers_url": format!("{url}/followers"),
        "following_url": format!("{url}/following{{/other_user}}"),
        "gists_url": format!("{url}/gists{{/gist_id}}"),
        "starred_url": format!("{url}/starred{{/owner}}{{/repo}}"),
        "subscriptions_url": format!("{url}/subscriptions"),
        "organizations_url": format!("{url}/orgs"),
        "repos_url": format!("{url}/repos"),
        "events_url": format!("{url}/events{{/privacy}}"),
        "received_events_url": format!("{url}/received_events"),
        "type": u.kind,
        "user_view_type": "public",
        "site_admin": u.site_admin,
        "name": opt_str(&u.name),
        "email": opt_str(&u.email),
    })
}

/// `public-user` schema (GET /users/{username}).
pub fn public(api: &str, base: &str, u: &User, public_repos: i64, followers: i64, following: i64) -> Value {
    let mut v = simple(api, base, u);
    let o = v.as_object_mut().unwrap();
    o.insert("company".into(), opt_str(&u.company));
    o.insert("blog".into(), json!(u.blog));
    o.insert("location".into(), opt_str(&u.location));
    o.insert("hireable".into(), Value::Null);
    o.insert("bio".into(), opt_str(&u.bio));
    o.insert("twitter_username".into(), opt_str(&u.twitter_username));
    o.insert("public_repos".into(), json!(public_repos));
    o.insert("public_gists".into(), json!(0));
    o.insert("followers".into(), json!(followers));
    o.insert("following".into(), json!(following));
    o.insert("created_at".into(), json!(ts(u.created_at)));
    o.insert("updated_at".into(), json!(ts(u.updated_at)));
    v
}

/// `private-user` schema (GET /user): public-user plus private counters.
pub fn private(api: &str, base: &str, u: &User, public_repos: i64, private_repos: i64) -> Value {
    let mut v = public(api, base, u, public_repos, 0, 0);
    let o = v.as_object_mut().unwrap();
    o.insert("private_gists".into(), json!(0));
    o.insert("total_private_repos".into(), json!(private_repos));
    o.insert("owned_private_repos".into(), json!(private_repos));
    o.insert("disk_usage".into(), json!(0));
    o.insert("collaborators".into(), json!(0));
    o.insert("two_factor_authentication".into(), json!(false));
    o.insert(
        "plan".into(),
        json!({"name": "enterprise", "space": 976562499, "private_repos": 9999, "collaborators": 0}),
    );
    v
}

/// `organization-simple` schema.
pub fn org_simple(api: &str, base: &str, u: &User) -> Value {
    let url = format!("{api}/orgs/{}", u.login);
    json!({
        "login": u.login,
        "id": u.id,
        "node_id": ids::encode("Organization", u.id),
        "url": url,
        "repos_url": format!("{url}/repos"),
        "events_url": format!("{url}/events"),
        "hooks_url": format!("{url}/hooks"),
        "issues_url": format!("{url}/issues"),
        "members_url": format!("{url}/members{{/member}}"),
        "public_members_url": format!("{url}/public_members{{/member}}"),
        "avatar_url": format!("{base}/avatars/{}", u.login),
        "description": opt_str(&u.bio),
    })
}

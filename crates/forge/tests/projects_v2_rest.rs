//! Projects (v2) REST API: projects, fields, items, item field values and views for users and
//! organizations, with GitHub's permission semantics. (GHES 3.22 has no classic-projects REST
//! API any more; the `projects` tag is Projects v2.)
mod common;
use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

async fn gql(s: &TestServer, u: &TestUser, query: &str, vars: Value) -> Value {
    let res = s
        .client()
        .post(format!("{}/api/graphql", s.base))
        .header("Authorization", format!("token {}", u.token))
        .json(&json!({"query": query, "variables": vars}))
        .send()
        .await
        .unwrap();
    let v: Value = res.json().await.unwrap();
    assert!(v.get("errors").is_none(), "{v}");
    v["data"].clone()
}

/// Creates a project for the owner with GraphQL node id `owner_node`; returns its number.
async fn create_project(s: &TestServer, u: &TestUser, owner_node: &str, title: &str) -> i64 {
    let d = gql(s, u, "mutation($o:ID!,$t:String!){createProjectV2(input:{ownerId:$o,title:$t}){projectV2{number}}}", json!({"o": owner_node, "t": title})).await;
    d["createProjectV2"]["projectV2"]["number"].as_i64().unwrap()
}

async fn me(s: &TestServer, u: &TestUser) -> Value {
    s.call(Some(u), "GET", "/user", None).await.1
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn user_project_fields_items_views() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let profile = me(&s, &u).await;
    let uid = profile["id"].as_i64().unwrap();
    let n = create_project(&s, &u, profile["node_id"].as_str().unwrap(), "Roadmap").await;
    let base = format!("/users/{}/projectsV2/{n}", u.login);

    // Project.
    let (st, p) = s.call(Some(&u), "GET", &base, None).await;
    assert_eq!(st, 200);
    assert_eq!(p["title"], "Roadmap");
    assert_eq!(p["state"], "open");
    assert_eq!(p["owner"]["login"], u.login.as_str());
    let (_, list) = s.call(Some(&u), "GET", &format!("/users/{}/projectsV2?q=road", u.login), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, list) = s.call(Some(&u), "GET", &format!("/users/{}/projectsV2?q=nothing", u.login), None).await;
    assert_eq!(list, json!([]));
    // Private project: invisible to others.
    let (st, _) = s.call(Some(&other), "GET", &base, None).await;
    assert_eq!(st, 404);
    let (_, list) = s.call(Some(&other), "GET", &format!("/users/{}/projectsV2", u.login), None).await;
    assert_eq!(list, json!([]));

    // Fields: GitHub's defaults plus custom ones.
    let (_, fields) = s.call(Some(&u), "GET", &format!("{base}/fields"), None).await;
    let names: Vec<&str> = fields.as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Title") && names.contains(&"Status"), "{names:?}");
    let status = fields.as_array().unwrap().iter().find(|f| f["name"] == "Status").unwrap().clone();
    assert_eq!(status["data_type"], "single_select");
    assert_eq!(status["options"][0]["name"]["raw"], "Todo");
    let (st, text) = s.call(Some(&u), "POST", &format!("{base}/fields"), Some(json!({"name": "Notes", "data_type": "text"}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/fields"), Some(json!({"name": "Notes", "data_type": "text"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/fields"), Some(json!({"name": "X", "data_type": "bogus"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/fields"), Some(json!({"name": "Sel", "data_type": "single_select", "single_select_options": []}))).await;
    assert_eq!(st, 422);
    let (st, num) = s.call(Some(&u), "POST", &format!("{base}/fields"), Some(json!({"name": "Points", "data_type": "number"}))).await;
    assert_eq!(st, 201);
    let (st, iter) = s
        .call(Some(&u), "POST", &format!("{base}/fields"), Some(json!({"name": "Sprint", "data_type": "iteration",
            "iteration_configuration": {"start_date": "2026-03-02", "duration": 7, "iterations": [{"title": "S1", "start_date": "2026-03-02", "duration": 7}, {"title": "S2", "start_date": "2026-03-09", "duration": 7}]}})))
        .await;
    assert_eq!(st, 201, "{iter}");
    assert_eq!(iter["configuration"]["duration"], 7);
    assert_eq!(iter["configuration"]["iterations"][1]["title"]["raw"], "S2");
    let (st, f) = s.call(Some(&u), "GET", &format!("{base}/fields/{}", text["id"]), None).await;
    assert_eq!((st, f["name"].clone()), (200, json!("Notes")));
    let (st, _) = s.call(Some(&other), "POST", &format!("{base}/fields"), Some(json!({"name": "Hack", "data_type": "text"}))).await;
    assert_eq!(st, 404);

    // Items: a draft and an issue.
    let (st, draft) = s.call(Some(&u), "POST", &format!("/user/{uid}/projectsV2/{n}/drafts"), Some(json!({"title": "Write docs", "body": "soon"}))).await;
    assert_eq!(st, 201, "{draft}");
    assert_eq!(draft["content_type"], "DraftIssue");
    assert_eq!(draft["content"]["title"], "Write docs");
    let repo = s.create_repo(&u, true).await;
    let (_, issue) = s.call(Some(&u), "POST", &format!("/repos/{}/{repo}/issues", u.login), Some(json!({"title": "Fix bug"}))).await;
    let (st, item) = s.call(Some(&u), "POST", &format!("{base}/items"), Some(json!({"type": "Issue", "owner": u.login, "repo": repo, "number": issue["number"]}))).await;
    assert_eq!(st, 201, "{item}");
    assert_eq!(item["content_type"], "Issue");
    assert_eq!(item["content"]["title"], "Fix bug");
    // By id, idempotent; wrong type refused.
    let (st, again) = s.call(Some(&u), "POST", &format!("{base}/items"), Some(json!({"type": "Issue", "id": issue["id"]}))).await;
    assert_eq!((st, again["id"].clone()), (201, item["id"].clone()));
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/items"), Some(json!({"type": "PullRequest", "id": issue["id"]}))).await;
    assert_eq!(st, 422);

    // Field values.
    let item_url = format!("{base}/items/{}", item["id"]);
    let todo = status["options"][0]["id"].clone();
    let s1 = iter["configuration"]["iterations"][0]["id"].clone();
    let (st, upd) = s
        .call(Some(&u), "PATCH", &item_url, Some(json!({"fields": [
            {"id": text["id"], "value": "hello"}, {"id": num["id"], "value": 3}, {"id": status["id"], "value": todo}, {"id": iter["id"], "value": s1}]})))
        .await;
    assert_eq!(st, 200, "{upd}");
    let vals: std::collections::HashMap<String, Value> = upd["fields"].as_array().unwrap().iter().map(|f| (f["name"].as_str().unwrap().to_string(), f["value"].clone())).collect();
    assert_eq!(vals["Notes"], "hello");
    assert_eq!(vals["Points"], 3.0);
    assert_eq!(vals["Status"]["name"]["raw"], "Todo");
    assert_eq!(vals["Sprint"]["title"]["raw"], "S1");
    let (st, _) = s.call(Some(&u), "PATCH", &item_url, Some(json!({"fields": [{"id": num["id"], "value": "not a number"}]}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "PATCH", &item_url, Some(json!({"fields": [{"id": status["id"], "value": "nope"}]}))).await;
    assert_eq!(st, 422);
    let (st, cleared) = s.call(Some(&u), "PATCH", &item_url, Some(json!({"fields": [{"id": text["id"], "value": null}]}))).await;
    assert_eq!(st, 200);
    assert_eq!(cleared["fields"][0]["value"], Value::Null);
    // Draft title through the Title field.
    let title = fields.as_array().unwrap().iter().find(|f| f["name"] == "Title").unwrap()["id"].clone();
    let (st, d) = s.call(Some(&u), "PATCH", &format!("{base}/items/{}", draft["id"]), Some(json!({"fields": [{"id": title, "value": "Write more docs"}]}))).await;
    assert_eq!(st, 200);
    assert_eq!(d["content"]["title"], "Write more docs");

    // Item reads with selected fields.
    let (st, got) = s.call(Some(&u), "GET", &format!("{item_url}?fields={},{}", status["id"], num["id"]), None).await;
    assert_eq!(st, 200);
    assert_eq!(got["fields"].as_array().unwrap().len(), 2);
    let (_, items) = s.call(Some(&u), "GET", &format!("{base}/items"), None).await;
    assert_eq!(items.as_array().unwrap().len(), 2);
    assert_eq!(items[0]["fields"][0]["data_type"], "title");
    let (_, items) = s.call(Some(&u), "GET", &format!("{base}/items?q=is:issue"), None).await;
    assert_eq!(items.as_array().unwrap().len(), 1);
    // Cursor pagination.
    let res = s.client().get(format!("{}{base}/items?per_page=1", s.api)).header("Authorization", format!("token {}", u.token)).send().await.unwrap();
    let link = res.headers().get("link").unwrap().to_str().unwrap().to_string();
    assert!(link.contains("rel=\"next\"") && link.contains("after="), "{link}");
    let next = link.split(['<', '>']).nth(1).unwrap().to_string();
    let page1: Value = res.json().await.unwrap();
    assert_eq!(page1.as_array().unwrap().len(), 1);
    let page2: Value = s.client().get(next).header("Authorization", format!("token {}", u.token)).send().await.unwrap().json().await.unwrap();
    assert_eq!(page2.as_array().unwrap().len(), 1);
    assert_ne!(page2[0]["id"], page1[0]["id"]);

    // Views.
    let (st, view) = s.call(Some(&u), "POST", &format!("/users/{uid}/projectsV2/{n}/views"), Some(json!({"name": "Issues", "layout": "table", "filter": "is:issue", "visible_fields": [num["id"]], "sort_by": [[num["id"], "desc"]]}))).await;
    assert_eq!(st, 201, "{view}");
    assert_eq!(view["number"], 2);
    assert_eq!(view["html_url"], format!("{}/users/{}/projects/{n}/views/2", s.base, u.login));
    let (st, _) = s.call(Some(&u), "POST", &format!("/users/{uid}/projectsV2/{n}/views"), Some(json!({"name": "R", "layout": "roadmap", "visible_fields": [num["id"]]}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("/users/{uid}/projectsV2/{n}/views"), Some(json!({"name": "B", "layout": "board", "group_by": [999999999]}))).await;
    assert_eq!(st, 422);
    let (st, vitems) = s.call(Some(&u), "GET", &format!("{base}/views/2/items"), None).await;
    assert_eq!(st, 200);
    assert_eq!(vitems.as_array().unwrap().len(), 1);
    assert_eq!(vitems[0]["fields"][0]["name"], "Points");
    let (_, all) = s.call(Some(&u), "GET", &format!("{base}/views/1/items"), None).await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/views/9/items"), None).await;
    assert_eq!(st, 404);

    // Delete.
    let (st, _) = s.call(Some(&other), "DELETE", &format!("{base}/items/{}", draft["id"]), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/items/{}", draft["id"]), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/items/{}", draft["id"]), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn org_project_permissions() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let outsider = s.user().await;
    let org = unique("org");
    let res = s.client().post(format!("{}/api/forge/orgs", s.base)).header("Authorization", format!("token {}", owner.token)).json(&json!({"login": org})).send().await.unwrap();
    assert!(res.status().is_success());
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/memberships/{}", member.login), Some(json!({"role": "member"}))).await;
    assert_eq!(st, 200);
    let (_, o) = s.call(Some(&owner), "GET", &format!("/orgs/{org}"), None).await;
    let n = create_project(&s, &owner, o["node_id"].as_str().unwrap(), "Org board").await;
    let base = format!("/orgs/{org}/projectsV2/{n}");

    let (st, p) = s.call(Some(&owner), "GET", &base, None).await;
    assert_eq!(st, 200);
    assert_eq!(p["owner"]["login"], org.as_str());
    // Members read, only admins write; outsiders see nothing.
    let (st, _) = s.call(Some(&member), "GET", &base, None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&member), "POST", &format!("{base}/drafts"), Some(json!({"title": "nope"}))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&member), "POST", &format!("{base}/views"), Some(json!({"name": "v", "layout": "board"}))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&outsider), "GET", &base, None).await;
    assert_eq!(st, 404);
    let (st, list) = s.call(Some(&outsider), "GET", &format!("/orgs/{org}/projectsV2"), None).await;
    assert_eq!((st, list), (200, json!([])));
    let (st, _) = s.call(None, "POST", &format!("{base}/drafts"), Some(json!({"title": "anon"}))).await;
    assert_eq!(st, 401);

    let (st, draft) = s.call(Some(&owner), "POST", &format!("{base}/drafts"), Some(json!({"title": "Plan"}))).await;
    assert_eq!(st, 201);
    assert_eq!(draft["project_url"], format!("{}{base}", s.api));
    assert_eq!(draft["item_url"], format!("{}{base}/items/{}", s.api, draft["id"]));
    let (st, _) = s.call(Some(&owner), "POST", &format!("{base}/drafts"), Some(json!({"body": "no title"}))).await;
    assert_eq!(st, 422);
    let (st, view) = s.call(Some(&owner), "POST", &format!("{base}/views"), Some(json!({"name": "Board", "layout": "board"}))).await;
    assert_eq!(st, 201);
    let (st, items) = s.call(Some(&member), "GET", &format!("{base}/views/{}/items", view["number"]), None).await;
    assert_eq!(st, 200);
    assert_eq!(items.as_array().unwrap().len(), 1);
    let (st, _) = s.call(Some(&member), "DELETE", &format!("{base}/items/{}", draft["id"]), None).await;
    assert_eq!(st, 403);
    // Issues of private repositories the viewer can't read are left out.
    let (_, repo) = s.call(Some(&owner), "POST", "/user/repos", Some(json!({"name": unique("r"), "private": true}))).await;
    let (_, issue) = s.call(Some(&owner), "POST", &format!("/repos/{}/{}/issues", owner.login, repo["name"].as_str().unwrap()), Some(json!({"title": "secret"}))).await;
    let (st, _) = s.call(Some(&owner), "POST", &format!("{base}/items"), Some(json!({"type": "Issue", "id": issue["id"]}))).await;
    assert_eq!(st, 201);
    let (_, items) = s.call(Some(&owner), "GET", &format!("{base}/items"), None).await;
    assert_eq!(items.as_array().unwrap().len(), 2);
    let (_, items) = s.call(Some(&member), "GET", &format!("{base}/items"), None).await;
    assert_eq!(items.as_array().unwrap().len(), 1);
    // The member can't add an issue they can't read.
    let (st, _) = s.call(Some(&member), "POST", &format!("{base}/items"), Some(json!({"type": "Issue", "id": issue["id"]}))).await;
    assert_eq!(st, 403);
}

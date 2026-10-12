//! Issues area: issues, comments, labels, milestones, events/timeline, reactions, search,
//! stars/watching and collaborators.
mod common;
use common::{TestServer, TestUser};
use serde_json::{json, Value};

async fn repo_json(s: &TestServer, u: &TestUser, repo: &str) -> Value {
    let (st, r) = s.call(Some(u), "GET", &format!("/repos/{}/{repo}", u.login), None).await;
    assert_eq!(st, 200);
    r
}

fn names(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn default_labels_and_label_crud() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, false).await;
    let base = format!("/repos/{}/{repo}", u.login);

    let (st, labels) = s.call(Some(&u), "GET", &format!("{base}/labels?per_page=100"), None).await;
    assert_eq!(st, 200);
    let n = names(&labels);
    assert_eq!(n.len(), 9, "{n:?}");
    assert!(n.contains(&"good first issue".to_string()));
    let bug = labels.as_array().unwrap().iter().find(|l| l["name"] == "bug").unwrap();
    assert_eq!(bug["color"], "d73a4a");
    assert_eq!(bug["default"], true);
    assert_eq!(bug["description"], "Something isn't working");

    let (st, l) = s.call(Some(&u), "POST", &format!("{base}/labels"), Some(json!({"name": "area/api", "color": "#FF0000"}))).await;
    assert_eq!(st, 201, "{l}");
    assert_eq!(l["color"], "ff0000");
    let (st, e) = s.call(Some(&u), "POST", &format!("{base}/labels"), Some(json!({"name": "AREA/api"}))).await;
    assert_eq!(st, 422);
    assert_eq!(e["errors"][0]["code"], "already_exists");
    let (st, l) = s
        .call(Some(&u), "PATCH", &format!("{base}/labels/help%20wanted"), Some(json!({"new_name": "help-wanted", "description": null})))
        .await;
    assert_eq!(st, 200, "{l}");
    assert_eq!(l["name"], "help-wanted");
    assert_eq!(l["description"], Value::Null);
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/labels/help%20wanted"), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/labels/wontfix"), None).await;
    assert_eq!(st, 204);

    // Pagination with Link headers.
    let res = s
        .client()
        .get(format!("{}{base}/labels?per_page=2", s.api))
        .header("Authorization", format!("token {}", u.token))
        .send()
        .await
        .unwrap();
    let link = res.headers()["link"].to_str().unwrap().to_string();
    assert!(link.contains("rel=\"next\"") && link.contains("rel=\"last\""), "{link}");

    // Other users can read but not write labels.
    let other = s.user().await;
    let (st, _) = s.call(Some(&other), "POST", &format!("{base}/labels"), Some(json!({"name": "x"}))).await;
    assert_eq!(st, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_lifecycle_counts_and_events() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, false).await;
    let base = format!("/repos/{}/{repo}", u.login);

    let (st, m) = s
        .call(Some(&u), "POST", &format!("{base}/milestones"), Some(json!({"title": "v1", "due_on": "2030-01-01T00:00:00Z"})))
        .await;
    assert_eq!(st, 201, "{m}");
    assert_eq!(m["number"], 1);

    let (st, i) = s
        .call(
            Some(&u),
            "POST",
            &format!("{base}/issues"),
            Some(json!({"title": "Crash on start", "body": "boom", "labels": ["bug", "brand-new"], "assignees": [u.login], "milestone": 1})),
        )
        .await;
    assert_eq!(st, 201, "{i}");
    assert_eq!(i["number"], 1);
    assert_eq!(i["state"], "open");
    assert_eq!(names(&i["labels"]), vec!["brand-new", "bug"]);
    assert_eq!(i["assignee"]["login"], u.login.as_str());
    assert_eq!(i["milestone"]["open_issues"], 1);
    assert_eq!(i["author_association"], "OWNER");
    assert_eq!(i["html_url"], format!("{}/{}/{repo}/issues/1", s.base, u.login));
    // The missing label was created for a user with push access.
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/labels/brand-new"), None).await;
    assert_eq!(st, 200);
    assert_eq!(repo_json(&s, &u, &repo).await["open_issues_count"], 1);

    // Validation.
    let (st, e) = s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"body": "no title"}))).await;
    assert_eq!(st, 422);
    assert_eq!(e["errors"][0]["field"], "title");
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "x", "milestone": 99}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "x", "assignees": ["nobody-here-zz"]}))).await;
    assert_eq!(st, 422);

    // Rename + close as not planned.
    let (st, i) = s
        .call(
            Some(&u),
            "PATCH",
            &format!("{base}/issues/1"),
            Some(json!({"title": "Crash on startup", "state": "closed", "state_reason": "not_planned", "milestone": null})),
        )
        .await;
    assert_eq!(st, 200, "{i}");
    assert_eq!(i["state"], "closed");
    assert_eq!(i["state_reason"], "not_planned");
    assert_eq!(i["closed_by"]["login"], u.login.as_str());
    assert!(i["closed_at"].is_string());
    assert_eq!(i["milestone"], Value::Null);
    assert_eq!(repo_json(&s, &u, &repo).await["open_issues_count"], 0);

    // Reopen.
    let (_, i) = s.call(Some(&u), "PATCH", &format!("{base}/issues/1"), Some(json!({"state": "open"}))).await;
    assert_eq!(i["state"], "open");
    assert_eq!(i["state_reason"], "reopened");
    assert_eq!(i["closed_at"], Value::Null);
    assert_eq!(repo_json(&s, &u, &repo).await["open_issues_count"], 1);

    // Labels and assignees endpoints.
    let (st, l) = s.call(Some(&u), "POST", &format!("{base}/issues/1/labels"), Some(json!({"labels": ["enhancement"]}))).await;
    assert_eq!(st, 200);
    assert_eq!(names(&l), vec!["brand-new", "bug", "enhancement"]);
    let (st, l) = s.call(Some(&u), "DELETE", &format!("{base}/issues/1/labels/bug"), None).await;
    assert_eq!(st, 200);
    assert_eq!(names(&l), vec!["brand-new", "enhancement"]);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/issues/1/labels/bug"), None).await;
    assert_eq!(st, 404);
    let (st, l) = s.call(Some(&u), "PUT", &format!("{base}/issues/1/labels"), Some(json!(["question"]))).await;
    assert_eq!(st, 200);
    assert_eq!(names(&l), vec!["question"]);
    let (st, i) = s.call(Some(&u), "DELETE", &format!("{base}/issues/1/assignees"), Some(json!({"assignees": [u.login]}))).await;
    assert_eq!(st, 200);
    assert_eq!(i["assignees"], json!([]));

    // Lock / unlock.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{base}/issues/1/lock"), Some(json!({"lock_reason": "too heated"}))).await;
    assert_eq!(st, 204);
    let (_, i) = s.call(Some(&u), "GET", &format!("{base}/issues/1"), None).await;
    assert_eq!(i["locked"], true);
    assert_eq!(i["active_lock_reason"], "too heated");
    let (st, _) = s.call(Some(&u), "PUT", &format!("{base}/issues/1/lock"), Some(json!({"lock_reason": "bogus"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/issues/1/lock"), None).await;
    assert_eq!(st, 204);

    let (st, ev) = s.call(Some(&u), "GET", &format!("{base}/issues/1/events?per_page=100"), None).await;
    assert_eq!(st, 200);
    let kinds: Vec<&str> = ev.as_array().unwrap().iter().map(|e| e["event"].as_str().unwrap()).collect();
    for k in ["labeled", "assigned", "milestoned", "renamed", "closed", "reopened", "demilestoned", "unlabeled", "unassigned", "locked", "unlocked"] {
        assert!(kinds.contains(&k), "missing {k} in {kinds:?}");
    }
    let renamed = ev.as_array().unwrap().iter().find(|e| e["event"] == "renamed").unwrap();
    assert_eq!(renamed["rename"], json!({"from": "Crash on start", "to": "Crash on startup"}));
    let closed = ev.as_array().unwrap().iter().find(|e| e["event"] == "closed").unwrap();
    assert_eq!(closed["state_reason"], "not_planned");

    // Repo-wide events embed the issue; single event by id.
    let (st, rev) = s.call(Some(&u), "GET", &format!("{base}/issues/events"), None).await;
    assert_eq!(st, 200);
    assert_eq!(rev[0]["issue"]["number"], 1);
    let id = rev[0]["id"].as_i64().unwrap();
    let (st, one) = s.call(Some(&u), "GET", &format!("{base}/issues/events/{id}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(one["id"], id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn comments_reactions_and_timeline() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, false).await;
    let base = format!("/repos/{}/{repo}", u.login);
    s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "first"}))).await;
    s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "second"}))).await;

    let (st, c) = s.call(Some(&other), "POST", &format!("{base}/issues/2/comments"), Some(json!({"body": "dup of #1"}))).await;
    assert_eq!(st, 201, "{c}");
    let cid = c["id"].as_i64().unwrap();
    assert_eq!(c["author_association"], "NONE");
    assert_eq!(c["html_url"], format!("{}/{}/{repo}/issues/2#issuecomment-{cid}", s.base, u.login));
    assert_eq!(c["reactions"]["total_count"], 0);
    let (_, c2) = s.call(Some(&u), "POST", &format!("{base}/issues/2/comments"), Some(json!({"body": "thanks"}))).await;
    let (_, i) = s.call(Some(&u), "GET", &format!("{base}/issues/2"), None).await;
    assert_eq!(i["comments"], 2);

    // Only the author or a writer may edit.
    let third = s.user().await;
    let (st, _) = s.call(Some(&third), "PATCH", &format!("{base}/issues/comments/{cid}"), Some(json!({"body": "hijack"}))).await;
    assert_eq!(st, 403);
    let (st, c) = s.call(Some(&other), "PATCH", &format!("{base}/issues/comments/{cid}"), Some(json!({"body": "dup of #1!"}))).await;
    assert_eq!(st, 200);
    assert_eq!(c["body"], "dup of #1!");

    // Repo-wide comment listing, sorted.
    let (st, all) = s.call(Some(&u), "GET", &format!("{base}/issues/comments?sort=created&direction=desc"), None).await;
    assert_eq!(st, 200);
    assert_eq!(all[0]["id"], c2["id"]);

    // Reactions: 201 new, 200 existing.
    let (st, r) = s.call(Some(&other), "POST", &format!("{base}/issues/2/reactions"), Some(json!({"content": "+1"}))).await;
    assert_eq!(st, 201, "{r}");
    let (st, r2) = s.call(Some(&other), "POST", &format!("{base}/issues/2/reactions"), Some(json!({"content": "+1"}))).await;
    assert_eq!(st, 200);
    assert_eq!(r["id"], r2["id"]);
    let (st, _) = s.call(Some(&other), "POST", &format!("{base}/issues/2/reactions"), Some(json!({"content": "nope"}))).await;
    assert_eq!(st, 422);
    let (_, i) = s.call(Some(&u), "GET", &format!("{base}/issues/2"), None).await;
    assert_eq!(i["reactions"]["+1"], 1);
    assert_eq!(i["reactions"]["total_count"], 1);
    let (st, rc) = s.call(Some(&u), "POST", &format!("{base}/issues/comments/{cid}/reactions"), Some(json!({"content": "heart"}))).await;
    assert_eq!(st, 201);
    let (_, list) = s.call(Some(&u), "GET", &format!("{base}/issues/comments/{cid}/reactions?content=heart"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, c) = s.call(Some(&u), "GET", &format!("{base}/issues/comments/{cid}"), None).await;
    assert_eq!(c["reactions"]["heart"], 1);
    let (st, _) = s.call(Some(&third), "DELETE", &format!("{base}/issues/comments/{cid}/reactions/{}", rc["id"]), None).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/issues/comments/{cid}/reactions/{}", rc["id"]), None).await;
    assert_eq!(st, 204);

    // The comment on #2 cross-references #1.
    let (st, tl) = s.call(Some(&u), "GET", &format!("{base}/issues/1/timeline"), None).await;
    assert_eq!(st, 200);
    let xr = tl.as_array().unwrap().iter().find(|e| e["event"] == "cross-referenced").expect("cross-reference");
    assert_eq!(xr["source"]["issue"]["number"], 2);
    let (_, tl2) = s.call(Some(&u), "GET", &format!("{base}/issues/2/timeline"), None).await;
    let commented: Vec<&Value> = tl2.as_array().unwrap().iter().filter(|e| e["event"] == "commented").collect();
    assert_eq!(commented.len(), 2);
    assert_eq!(commented[0]["actor"]["login"], other.login.as_str());

    // Delete a comment: count drops.
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/issues/comments/{}", c2["id"]), None).await;
    assert_eq!(st, 204);
    let (_, i) = s.call(Some(&u), "GET", &format!("{base}/issues/2"), None).await;
    assert_eq!(i["comments"], 1);

    // Locked conversations reject comments from non-collaborators.
    s.call(Some(&u), "PUT", &format!("{base}/issues/2/lock"), None).await;
    let (st, _) = s.call(Some(&other), "POST", &format!("{base}/issues/2/comments"), Some(json!({"body": "late"}))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/issues/2/comments"), Some(json!({"body": "owner can"}))).await;
    assert_eq!(st, 201);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn list_filters_and_permissions() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, false).await;
    let base = format!("/repos/{}/{repo}", u.login);
    s.call(Some(&u), "POST", &format!("{base}/milestones"), Some(json!({"title": "m1"}))).await;
    s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "a", "labels": ["bug"], "assignees": [u.login], "milestone": 1}))).await;
    s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "b", "labels": ["bug", "question"], "body": "ping @someone"}))).await;
    // A non-collaborator can open an issue, but labels are silently dropped.
    let (st, i) = s.call(Some(&other), "POST", &format!("{base}/issues"), Some(json!({"title": "c", "labels": ["bug"]}))).await;
    assert_eq!(st, 201);
    assert_eq!(i["labels"], json!([]));
    // ... and can't edit someone else's issue, but can close their own.
    let (st, _) = s.call(Some(&other), "PATCH", &format!("{base}/issues/1"), Some(json!({"title": "mine now"}))).await;
    assert_eq!(st, 403);
    let (st, i) = s.call(Some(&other), "PATCH", &format!("{base}/issues/3"), Some(json!({"state": "closed", "labels": ["bug"]}))).await;
    assert_eq!(st, 200);
    assert_eq!(i["labels"], json!([]));

    let list = |q: &str| format!("{base}/issues?{q}");
    let numbers = |v: &Value| v.as_array().unwrap().iter().map(|i| i["number"].as_i64().unwrap()).collect::<Vec<_>>();
    let (_, v) = s.call(Some(&u), "GET", &list(""), None).await;
    assert_eq!(numbers(&v), vec![2, 1]);
    let (_, v) = s.call(Some(&u), "GET", &list("state=all&direction=asc"), None).await;
    assert_eq!(numbers(&v), vec![1, 2, 3]);
    let (_, v) = s.call(Some(&u), "GET", &list("labels=bug,question"), None).await;
    assert_eq!(numbers(&v), vec![2]);
    let (_, v) = s.call(Some(&u), "GET", &list("assignee=none"), None).await;
    assert_eq!(numbers(&v), vec![2]);
    let (_, v) = s.call(Some(&u), "GET", &list(&format!("assignee={}", u.login)), None).await;
    assert_eq!(numbers(&v), vec![1]);
    let (_, v) = s.call(Some(&u), "GET", &list(&format!("creator={}&state=closed", other.login)), None).await;
    assert_eq!(numbers(&v), vec![3]);
    let (_, v) = s.call(Some(&u), "GET", &list("milestone=1"), None).await;
    assert_eq!(numbers(&v), vec![1]);
    let (_, v) = s.call(Some(&u), "GET", &list("milestone=none"), None).await;
    assert_eq!(numbers(&v), vec![2]);
    let (_, v) = s.call(Some(&u), "GET", &list("mentioned=someone"), None).await;
    assert_eq!(numbers(&v), vec![2]);
    let (st, _) = s.call(Some(&u), "GET", &list("state=bogus"), None).await;
    assert_eq!(st, 422);

    // Pagination (cursor-like: next without last on full pages).
    let res = s
        .client()
        .get(format!("{}{}", s.api, list("per_page=1")))
        .header("Authorization", format!("token {}", u.token))
        .send()
        .await
        .unwrap();
    assert!(res.headers()["link"].to_str().unwrap().contains("rel=\"next\""));

    // Milestone counts.
    let (_, m) = s.call(Some(&u), "GET", &format!("{base}/milestones/1"), None).await;
    assert_eq!((m["open_issues"].as_i64(), m["closed_issues"].as_i64()), (Some(1), Some(0)));
    let (_, ml) = s.call(Some(&u), "GET", &format!("{base}/milestones/1/labels"), None).await;
    assert_eq!(names(&ml), vec!["bug"]);

    // Cross-repo listings.
    let (st, v) = s.call(Some(&u), "GET", "/issues?filter=assigned", None).await;
    assert_eq!(st, 200);
    assert_eq!(numbers(&v), vec![1]);
    assert_eq!(v[0]["repository"]["name"], repo.as_str());
    let (_, v) = s.call(Some(&u), "GET", "/user/issues?filter=all&state=all", None).await;
    assert_eq!(v.as_array().unwrap().len(), 3);
    let (_, v) = s.call(Some(&other), "GET", "/issues?filter=created&state=all", None).await;
    assert_eq!(v.as_array().unwrap().len(), 0, "other is not a member of the repo");

    // Assignees: owner yes, stranger no; collaborators become assignable.
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/assignees/{}", u.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/assignees/{}", other.login), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{base}/collaborators/{}", other.login), Some(json!({"permission": "triage"}))).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/assignees/{}", other.login), None).await;
    assert_eq!(st, 204);
    let (st, p) = s.call(Some(&u), "GET", &format!("{base}/collaborators/{}/permission", other.login), None).await;
    assert_eq!(st, 200);
    assert_eq!(p["permission"], "read");
    assert_eq!(p["role_name"], "triage");
    let (_, c) = s.call(Some(&u), "GET", &format!("{base}/collaborators"), None).await;
    assert_eq!(c.as_array().unwrap().len(), 2);
    assert_eq!(c[0]["role_name"], "admin");
    // Triage can now label issues it didn't open.
    let (st, l) = s.call(Some(&other), "POST", &format!("{base}/issues/1/labels"), Some(json!(["question"]))).await;
    assert_eq!(st, 200, "{l}");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/collaborators/{}", other.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/collaborators/{}", other.login), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_issues_repos_users() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, false).await;
    let base = format!("/repos/{}/{repo}", u.login);
    s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "Parser panics on unicode", "labels": ["bug"]}))).await;
    s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "Add dark mode", "body": "please", "labels": ["help wanted"]}))).await;
    s.call(Some(&other), "POST", &format!("{base}/issues"), Some(json!({"title": "Docs typo"}))).await;
    s.call(Some(&u), "PATCH", &format!("{base}/issues/3"), Some(json!({"state": "closed"}))).await;
    s.call(Some(&other), "POST", &format!("{base}/issues/1/comments"), Some(json!({"body": "same here"}))).await;

    let r = format!("repo:{}/{repo}", u.login);
    assert_eq!(search(&s, &u, format!("{r} is:issue")).await, vec![1, 2, 3]);
    assert_eq!(search(&s, &u, format!("{r} is:open")).await, vec![1, 2]);
    assert_eq!(search(&s, &u, format!("{r} state:closed")).await, vec![3]);
    assert_eq!(search(&s, &u, format!("{r} label:bug")).await, vec![1]);
    assert_eq!(search(&s, &u, format!("{r} label:\"help wanted\"")).await, vec![2]);
    assert_eq!(search(&s, &u, format!("{r} -label:bug is:open")).await, vec![2]);
    assert_eq!(search(&s, &u, format!("{r} no:label")).await, vec![3]);
    assert_eq!(search(&s, &u, format!("{r} author:{}", other.login)).await, vec![3]);
    assert_eq!(search(&s, &u, format!("{r} commenter:{}", other.login)).await, vec![1]);
    assert_eq!(search(&s, &u, format!("{r} involves:{}", other.login)).await, vec![1, 3]);
    assert_eq!(search(&s, &u, format!("{r} author:@me is:open")).await, vec![1, 2]);
    assert_eq!(search(&s, &u, format!("{r} unicode")).await, vec![1]);
    assert_eq!(search(&s, &u, format!("{r} pars in:title")).await, vec![1]);
    assert_eq!(search(&s, &u, format!("{r} please in:body")).await, vec![2]);
    assert_eq!(search(&s, &u, format!("{r} is:pr")).await, Vec::<i64>::new());
    assert_eq!(search(&s, &u, format!("{r} created:>=2000-01-01 comments:>0")).await, vec![1]);
    assert_eq!(search(&s, &u, format!("user:{} dark", u.login)).await, vec![2]);

    // Items carry score; missing q is a 422.
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/issues?q={}", url_encode(&format!("{r} dark"))), None).await;
    assert_eq!(v["items"][0]["score"], 1.0);
    let (st, _) = s.call(Some(&u), "GET", "/search/issues", None).await;
    assert_eq!(st, 422);

    // Private repositories are invisible to others.
    let (_, pr) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": format!("{repo}-secret"), "private": true}))).await;
    let secret = pr["name"].as_str().unwrap().to_string();
    s.call(Some(&u), "POST", &format!("/repos/{}/{secret}/issues", u.login), Some(json!({"title": "hidden thing"}))).await;
    let q = url_encode(&format!("user:{} hidden", u.login));
    let (_, v) = s.call(Some(&other), "GET", &format!("/search/issues?q={q}"), None).await;
    assert_eq!(v["total_count"], 0);
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/issues?q={q}"), None).await;
    assert_eq!(v["total_count"], 1);

    // Repositories and users.
    let (_, v) = s.call(Some(&other), "GET", &format!("/search/repositories?q={}", url_encode(&format!("{repo} user:{}", u.login))), None).await;
    assert_eq!(v["total_count"], 1, "{v}");
    assert_eq!(v["items"][0]["name"], repo.as_str());
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/repositories?q={}", url_encode(&format!("user:{} is:private", u.login))), None).await;
    assert_eq!(v["total_count"], 1);
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/users?q={}", other.login), None).await;
    assert_eq!(v["items"][0]["login"], other.login.as_str());
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/users?q={}+type:org", other.login), None).await;
    assert_eq!(v["total_count"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn code_and_commit_search() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let r = format!("repo:{}/{repo}", u.login);
    let (st, v) = s.call(Some(&u), "GET", &format!("/search/code?q={}", url_encode(&format!("{r} {repo}"))), None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["total_count"], 1);
    assert_eq!(v["items"][0]["path"], "README.md");
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/code?q={}", url_encode(&format!("{r} {repo} extension:rs"))), None).await;
    assert_eq!(v["total_count"], 0);
    let (st, v) = s.call(Some(&u), "GET", &format!("/search/commits?q={}", url_encode(&format!("{r} initial"))), None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["total_count"], 1);
    assert_eq!(v["items"][0]["commit"]["message"], "Initial commit");
    assert_eq!(v["items"][0]["author"]["login"], u.login.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stars_and_subscriptions() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let fan = s.user().await;
    let repo = s.create_repo(&u, false).await;
    let base = format!("/repos/{}/{repo}", u.login);
    let starred = format!("/user/starred/{}/{repo}", u.login);

    let (st, _) = s.call(Some(&fan), "GET", &starred, None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&fan), "PUT", &starred, None).await;
    assert_eq!(st, 204);
    s.call(Some(&fan), "PUT", &starred, None).await; // idempotent
    let (st, _) = s.call(Some(&fan), "GET", &starred, None).await;
    assert_eq!(st, 204);
    assert_eq!(repo_json(&s, &u, &repo).await["stargazers_count"], 1);
    let (_, g) = s.call(Some(&u), "GET", &format!("{base}/stargazers"), None).await;
    assert_eq!(g[0]["login"], fan.login.as_str());
    let res = s
        .client()
        .get(format!("{}{base}/stargazers", s.api))
        .header("Accept", "application/vnd.github.star+json")
        .send()
        .await
        .unwrap();
    let g: Value = res.json().await.unwrap();
    assert!(g[0]["starred_at"].is_string());
    assert_eq!(g[0]["user"]["login"], fan.login.as_str());
    let (_, l) = s.call(None, "GET", &format!("/users/{}/starred", fan.login), None).await;
    assert_eq!(l[0]["name"], repo.as_str());
    let (st, _) = s.call(Some(&fan), "DELETE", &starred, None).await;
    assert_eq!(st, 204);
    assert_eq!(repo_json(&s, &u, &repo).await["stargazers_count"], 0);

    // The creator watches their new repository.
    let (st, w) = s.call(Some(&u), "GET", &format!("{base}/subscription"), None).await;
    assert_eq!(st, 200);
    assert_eq!(w["subscribed"], true);
    let (st, _) = s.call(Some(&fan), "GET", &format!("{base}/subscription"), None).await;
    assert_eq!(st, 404);
    let (st, w) = s.call(Some(&fan), "PUT", &format!("{base}/subscription"), Some(json!({"subscribed": true}))).await;
    assert_eq!(st, 200);
    assert_eq!(w["ignored"], false);
    let (_, subs) = s.call(Some(&u), "GET", &format!("{base}/subscribers"), None).await;
    assert_eq!(subs.as_array().unwrap().len(), 2);
    let (_, mine) = s.call(Some(&fan), "GET", "/user/subscriptions", None).await;
    assert_eq!(mine[0]["name"], repo.as_str());
    let (st, _) = s.call(Some(&fan), "DELETE", &format!("{base}/subscription"), None).await;
    assert_eq!(st, 204);
    let (_, subs) = s.call(Some(&u), "GET", &format!("{base}/subscribers"), None).await;
    assert_eq!(subs.as_array().unwrap().len(), 1);
}

async fn search(s: &TestServer, u: &TestUser, q: String) -> Vec<i64> {
    let url = format!("/search/issues?q={}", url_encode(&q));
    let (st, v) = s.call(Some(u), "GET", &url, None).await;
    assert_eq!(st, 200, "{q}: {v}");
    assert_eq!(v["incomplete_results"], false);
    let mut n: Vec<i64> = v["items"].as_array().unwrap().iter().map(|i| i["number"].as_i64().unwrap()).collect();
    n.sort();
    assert_eq!(v["total_count"].as_i64().unwrap() as usize, n.len(), "{q}");
    n
}

#[test]
fn reference_parsing() {
    use forge::api::issues::{closing_numbers, referenced_numbers};
    assert_eq!(referenced_numbers("see #1, #22 and a#3 &#4; (#5)"), vec![1, 22, 5]);
    assert_eq!(closing_numbers("Fixes #1, closes: #2\nresolved #3 and mentions #4"), vec![1, 2, 3]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_references_and_closes_issues() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let base = format!("/repos/{}/{repo}", u.login);
    s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "one"}))).await;
    s.call(Some(&u), "POST", &format!("{base}/issues"), Some(json!({"title": "two"}))).await;

    let w = s.work_dir("refs");
    let (ok, out) = s.git(&w, &["clone", &s.clone_url(&u, &repo), "c"]);
    assert!(ok, "{out}");
    let c = w.join("c");
    std::fs::write(c.join("f.txt"), "x").unwrap();
    assert!(s.git(&c, &["add", "."]).0);
    assert!(s.git(&c, &["commit", "-m", "Fixes #1, see #2"]).0);
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD"]);
    assert!(ok, "{out}");

    let (_, i1) = s.call(Some(&u), "GET", &format!("{base}/issues/1"), None).await;
    assert_eq!(i1["state"], "closed");
    assert_eq!(i1["state_reason"], "completed");
    let (_, i2) = s.call(Some(&u), "GET", &format!("{base}/issues/2"), None).await;
    assert_eq!(i2["state"], "open");
    let (_, ev) = s.call(Some(&u), "GET", &format!("{base}/issues/2/events"), None).await;
    let r = ev.as_array().unwrap().iter().find(|e| e["event"] == "referenced").expect("referenced event");
    assert_eq!(r["commit_id"].as_str().unwrap().len(), 40);
    assert!(r["commit_url"].as_str().unwrap().contains("/commits/"));
    assert_eq!(r["actor"]["login"], u.login.as_str());
}

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

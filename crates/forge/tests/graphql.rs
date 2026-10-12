//! GraphQL (/api/graphql) integration tests: gh's queries, GitHub leniency, errors, mutations.
mod common;
use common::{TestServer, TestUser};
use serde_json::{json, Value};

/// POSTs a raw GraphQL body; returns (status, json).
async fn gql_raw(s: &TestServer, u: Option<&TestUser>, body: Value) -> (u16, Value) {
    let mut req = s.client().post(format!("{}/api/graphql", s.base)).json(&body);
    if let Some(u) = u {
        req = req.header("Authorization", format!("token {}", u.token));
    }
    let res = req.send().await.unwrap();
    let st = res.status().as_u16();
    (st, res.json().await.unwrap())
}

async fn gql(s: &TestServer, u: &TestUser, query: &str, vars: Value) -> Value {
    let (st, v) = gql_raw(s, Some(u), json!({"query": query, "variables": vars})).await;
    assert_eq!(st, 200, "{v}");
    v
}

/// Like `gql` but asserts there are no errors and returns `data`.
async fn data(s: &TestServer, u: &TestUser, query: &str, vars: Value) -> Value {
    let v = gql(s, u, query, vars).await;
    assert!(v.get("errors").is_none(), "unexpected errors: {v}");
    v["data"].clone()
}

async fn repo_id(s: &TestServer, u: &TestUser, name: &str) -> String {
    let d = data(s, u, "query($o:String!,$n:String!){repository(owner:$o,name:$n){id}}", json!({"o": u.login, "n": name})).await;
    d["repository"]["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn viewer_auth_and_errors() {
    let s = TestServer::start().await;
    let u = s.user().await;

    // gh auth login: exact body with "variables": null.
    let (st, v) = gql_raw(&s, Some(&u), json!({"query": "query UserCurrent{viewer{login}}", "variables": null})).await;
    assert_eq!(st, 200);
    assert_eq!(v, json!({"data": {"viewer": {"login": u.login}}}));
    // Without a variables key at all.
    let (_, v) = gql_raw(&s, Some(&u), json!({"query": "query UserCurrent{viewer{login,id}}"})).await;
    assert!(v["data"]["viewer"]["id"].as_str().unwrap().starts_with("U_"));

    // Anonymous: 401 like GitHub.
    let (st, v) = gql_raw(&s, None, json!({"query": "{viewer{login}}"})).await;
    assert_eq!(st, 401);
    assert!(v["message"].as_str().unwrap().contains("requires you to be authenticated"));

    // Missing repository: HTTP 200, data.repository null, top-level type NOT_FOUND.
    let v = gql(&s, &u, "query($o:String!,$n:String!){repository(owner:$o,name:$n){name}}", json!({"o": u.login, "n": "nope"})).await;
    assert_eq!(v["data"]["repository"], Value::Null);
    let e = &v["errors"][0];
    assert_eq!(e["type"], "NOT_FOUND");
    assert_eq!(e["path"], json!(["repository"]));
    assert_eq!(e["message"], format!("Could not resolve to a Repository with the name '{}/nope'.", u.login));

    // Only requested fields come back (gh's decoder rejects extra keys), aliases honoured.
    let d = data(&s, &u, "{me: viewer{login}}", json!({})).await;
    assert_eq!(d, json!({"me": {"login": u.login}}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn introspection_matches_minimal_ghes_profile() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let names = |v: &Value, ty: &str| -> Vec<String> {
        v[ty]["fields"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_string()).collect()
    };
    let d = data(&s, &u, r#"query Issue_fields{Issue: __type(name: "Issue"){fields(includeDeprecated: true){name}}}"#, Value::Null).await;
    let f = names(&d, "Issue");
    assert!(f.contains(&"stateReason".to_string()) && !f.contains(&"blockedBy".to_string()));
    let d = data(
        &s,
        &u,
        r#"query PullRequest_fields{PullRequest: __type(name: "PullRequest"){fields(includeDeprecated: true){name}},StatusCheckRollupContextConnection: __type(name: "StatusCheckRollupContextConnection"){fields(includeDeprecated: true){name}}}"#,
        Value::Null,
    )
    .await;
    assert!(names(&d, "PullRequest").contains(&"mergeStateStatus".to_string()));
    assert!(!names(&d, "PullRequest").contains(&"isInMergeQueue".to_string()));
    assert!(!names(&d, "StatusCheckRollupContextConnection").contains(&"checkRunCount".to_string()));
    let d = data(&s, &u, r#"query PullRequest_fields2{WorkflowRun: __type(name: "WorkflowRun"){fields(includeDeprecated: true){name}}}"#, Value::Null).await;
    assert!(!names(&d, "WorkflowRun").contains(&"event".to_string()));
    let d = data(&s, &u, r#"query Repository_fields{Repository: __type(name: "Repository"){fields(includeDeprecated: true){name}}}"#, Value::Null).await;
    let f = names(&d, "Repository");
    assert!(f.contains(&"visibility".to_string()) && !f.contains(&"pullRequestTemplates".to_string()) && !f.contains(&"autoMergeAllowed".to_string()));
    let d = data(&s, &u, r#"query Release_fields{Release: __type(name: "Release"){fields{name}}}"#, Value::Null).await;
    assert!(!names(&d, "Release").contains(&"immutable".to_string()));
    let d = data(&s, &u, r#"query SearchType_enumValues{SearchType: __type(name: "SearchType"){enumValues(includeDeprecated: true){name}}}"#, Value::Null).await;
    let vals: Vec<&str> = d["SearchType"]["enumValues"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert!(vals.contains(&"ISSUE") && !vals.contains(&"ISSUE_ADVANCED"));
    let d = data(&s, &u, r#"query ProjectV2_fields{ProjectV2: __type(name: "ProjectV2"){fields(includeDeprecated: true){name,args{name}}}}"#, Value::Null).await;
    assert!(d["ProjectV2"]["fields"].is_array());
    // Unknown types are null, not an error.
    let d = data(&s, &u, r#"{X: __type(name: "NoSuchType"){name}}"#, Value::Null).await;
    assert_eq!(d["X"], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issues_lenient_queries_and_mutations() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rid = repo_id(&s, &u, &repo).await;

    // gh issue create (string mutation, null labelIds/projectIds).
    let mut numbers = Vec::new();
    for t in ["one", "two", "three"] {
        let d = data(
            &s,
            &u,
            "\n\tmutation IssueCreate($input: CreateIssueInput!) {\n\t\tcreateIssue(input: $input) {\n\t\t\tissue {\n\t\t\t\tid\n\t\t\t\turl\n\t\t\t}\n\t\t}\n\t}",
            json!({"input": {"repositoryId": rid, "title": t, "body": "b", "labelIds": null, "projectIds": null}}),
        )
        .await;
        let url = d["createIssue"]["issue"]["url"].as_str().unwrap().to_string();
        numbers.push(url.rsplit('/').next().unwrap().parse::<i64>().unwrap());
    }
    assert_eq!(numbers, vec![1, 2, 3]);

    // IssueByNumber: Issue.state vs PullRequest.state overlap (strict validators reject this).
    let q = "query IssueByNumber($owner: String!, $repo: String!, $number: Int!) { repository(owner: $owner, name: $repo) { hasIssuesEnabled issue: issueOrPullRequest(number: $number) { __typename ...on Issue{id,number,title,state,stateReason,labels(first:100){nodes{id,name},totalCount},comments(last: 1){nodes{body},totalCount}} ...on PullRequest{id,number,title,state} } } }";
    let d = data(&s, &u, q, json!({"owner": u.login, "repo": repo, "number": 1})).await;
    let issue = &d["repository"]["issue"];
    assert_eq!(issue["__typename"], "Issue");
    assert_eq!(issue["state"], "OPEN");
    let issue_id = issue["id"].as_str().unwrap().to_string();
    // Missing number: NOT_FOUND on the path gh matches.
    let v = gql(&s, &u, q, json!({"owner": u.login, "repo": repo, "number": 99})).await;
    assert_eq!(v["errors"][0]["type"], "NOT_FOUND");
    assert_eq!(v["errors"][0]["path"], json!(["repository", "issue"]));

    // addComment, struct-generated text, `ID!` variable.
    let d = data(
        &s,
        &u,
        "mutation CommentCreate($input:AddCommentInput!){addComment(input: $input){commentEdge{node{url}}}}",
        json!({"input": {"subjectId": issue_id, "body": "hello"}}),
    )
    .await;
    assert!(d["addComment"]["commentEdge"]["node"]["url"].as_str().unwrap().contains("#issuecomment-"));

    // closeIssue with reason, then reopen.
    data(&s, &u, "mutation IssueClose($input:CloseIssueInput!){closeIssue(input: $input){issue{id}}}", json!({"input": {"issueId": issue_id, "stateReason": "NOT_PLANNED"}})).await;
    let d = data(&s, &u, q, json!({"owner": u.login, "repo": repo, "number": 1})).await;
    assert_eq!(d["repository"]["issue"]["state"], "CLOSED");
    assert_eq!(d["repository"]["issue"]["stateReason"], "NOT_PLANNED");
    data(&s, &u, "mutation IssueReopen($input:ReopenIssueInput!){reopenIssue(input: $input){issue{id}}}", json!({"input": {"issueId": issue_id}})).await;

    // Labels: createLabel + addLabelsToLabelable, read back.
    let d = data(
        &s,
        &u,
        "mutation($r: ID!){createLabel(input:{repositoryId:$r,name:\"gql\",color:\"00ff00\"}){label{id name color}}}",
        json!({"r": rid}),
    )
    .await;
    let label_id = d["createLabel"]["label"]["id"].as_str().unwrap().to_string();
    data(
        &s,
        &u,
        "mutation LabelAdd($input:AddLabelsToLabelableInput!){addLabelsToLabelable(input: $input){__typename}}",
        json!({"input": {"labelableId": issue_id, "labelIds": [label_id]}}),
    )
    .await;
    let d = data(&s, &u, q, json!({"owner": u.login, "repo": repo, "number": 1})).await;
    assert_eq!(d["repository"]["issue"]["labels"]["nodes"][0]["name"], "gql");
    assert_eq!(d["repository"]["issue"]["comments"]["totalCount"], 1);

    // gh issue list: absent nullable variables ($assignee...) and variable defaults ($states = OPEN).
    let list = "fragment issue on Issue {number,title,state}\n\tquery IssueList($owner: String!, $repo: String!, $limit: Int, $endCursor: String, $states: [IssueState!] = OPEN, $assignee: String, $author: String, $mention: String) { repository(owner: $owner, name: $repo) { hasIssuesEnabled issues(first: $limit, after: $endCursor, orderBy: {field: CREATED_AT, direction: DESC}, states: $states, filterBy: {assignee: $assignee, createdBy: $author, mentioned: $mention}) { totalCount nodes { ...issue } pageInfo { hasNextPage endCursor } } } }";
    let d = data(&s, &u, list, json!({"owner": u.login, "repo": repo, "limit": 2})).await;
    let conn = &d["repository"]["issues"];
    assert_eq!(conn["totalCount"], 3);
    assert_eq!(conn["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(conn["nodes"][0]["number"], 3);
    assert_eq!(conn["pageInfo"]["hasNextPage"], true);
    // Next page via the real cursor, plus an undeclared variable (gh sends `perPage`).
    let cur = conn["pageInfo"]["endCursor"].as_str().unwrap();
    let d = data(&s, &u, list, json!({"owner": u.login, "repo": repo, "limit": 2, "endCursor": cur, "perPage": 2})).await;
    let conn = &d["repository"]["issues"];
    assert_eq!(conn["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(conn["nodes"][0]["number"], 1);
    assert_eq!(conn["pageInfo"]["hasNextPage"], false);

    // Search (gh's IssueSearch uses `last:` and advanced syntax).
    let d = data(
        &s,
        &u,
        "query($q: String!){search(type: ISSUE, last: 10, query: $q){issueCount nodes{...on Issue{number}}}}",
        json!({"q": format!("( two ) repo:{}/{} state:open type:issue", u.login, repo)}),
    )
    .await;
    assert_eq!(d["search"]["issueCount"], 1);
    assert_eq!(d["search"]["nodes"][0]["number"], 2);

    // Reactions show up in reactionGroups (all 8 groups, counts per content).
    data(&s, &u, "mutation($i: ID!){addReaction(input:{subjectId:$i,content:HEART}){reaction{content}}}", json!({"i": issue_id})).await;
    let d = data(&s, &u, "query($id: ID!){node(id: $id){...on Issue{reactionGroups{content,users{totalCount}}}}}", json!({"id": issue_id})).await;
    let groups = d["node"]["reactionGroups"].as_array().unwrap();
    assert_eq!(groups.len(), 8);
    assert!(groups.iter().any(|g| g["content"] == "HEART" && g["users"]["totalCount"] == 1));

    // node(id) and deleteIssue.
    let d = data(&s, &u, "query($id: ID!){node(id: $id){__typename ...on Issue{number}}}", json!({"id": issue_id})).await;
    assert_eq!(d["node"]["number"], 1);
    data(&s, &u, "mutation($i: ID!){deleteIssue(input:{issueId:$i}){clientMutationId}}", json!({"i": issue_id})).await;
    let v = gql(&s, &u, "query($id: ID!){node(id: $id){id}}", json!({"id": issue_id})).await;
    assert_eq!(v["errors"][0]["type"], "NOT_FOUND");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_repository_and_list() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let name = common::unique("gq");
    let d = data(
        &s,
        &u,
        "\n\tmutation RepositoryCreate($input: CreateRepositoryInput!) {\n\t\tcreateRepository(input: $input) {\n\t\t\trepository {\n\t\t\t\tid\n\t\t\t\tname\n\t\t\t\towner { login }\n\t\t\t\turl\n\t\t\t}\n\t\t}\n\t}",
        json!({"input": {"name": name, "visibility": "PRIVATE", "description": "d", "hasIssuesEnabled": true, "hasWikiEnabled": true}}),
    )
    .await;
    assert_eq!(d["createRepository"]["repository"]["owner"]["login"], u.login);
    assert_eq!(d["createRepository"]["repository"]["url"], format!("{}/{}/{}", s.base, u.login, name));
    // gh repo list (aliased viewer as repositoryOwner, absent $privacy/$fork).
    let d = data(
        &s,
        &u,
        "query RepositoryList($perPage:Int!,$endCursor:String,$privacy:RepositoryPrivacy,$fork:Boolean) { repositoryOwner: viewer { login repositories(first: $perPage, after: $endCursor, privacy: $privacy, isFork: $fork, ownerAffiliations: OWNER, orderBy: { field: PUSHED_AT, direction: DESC }) { nodes{nameWithOwner,isPrivate,visibility,isFork,createdAt,pushedAt} totalCount pageInfo{hasNextPage,endCursor} } } }",
        json!({"perPage": 30}),
    )
    .await;
    let nodes = d["repositoryOwner"]["repositories"]["nodes"].as_array().unwrap();
    let n = nodes.iter().find(|n| n["nameWithOwner"] == format!("{}/{}", u.login, name)).unwrap();
    assert_eq!(n["visibility"], "PRIVATE");
    assert_eq!(n["isPrivate"], true);
    // Another user can't see it.
    let other = s.user().await;
    let v = gql(&s, &other, "query($o:String!,$n:String!){repository(owner:$o,name:$n){id}}", json!({"o": u.login, "n": name})).await;
    assert_eq!(v["errors"][0]["type"], "NOT_FOUND");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_request_create_review_merge() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let reviewer = s.user().await;
    let repo = s.create_repo(&u, true).await;
    s.call(Some(&u), "PUT", &format!("/repos/{}/{}/collaborators/{}", u.login, repo, reviewer.login), Some(json!({"permission": "push"}))).await;
    let rid = repo_id(&s, &u, &repo).await;

    // Push a topic branch with two commits.
    let w = s.work_dir("pr");
    let (ok, out) = s.git(&w, &["clone", &s.clone_url(&u, &repo), "c"]);
    assert!(ok, "{out}");
    let c = w.join("c");
    assert!(s.git(&c, &["checkout", "-b", "topic"]).0);
    std::fs::write(c.join("a.txt"), "a\n").unwrap();
    assert!(s.git(&c, &["add", "."]).0);
    assert!(s.git(&c, &["commit", "-m", "Add a"]).0);
    std::fs::write(c.join("b.txt"), "b\n").unwrap();
    assert!(s.git(&c, &["add", "."]).0);
    assert!(s.git(&c, &["commit", "-m", "Add b"]).0);
    let (ok, out) = s.git(&c, &["push", "origin", "topic"]);
    assert!(ok, "{out}");

    let d = data(
        &s,
        &u,
        "mutation PullRequestCreate($input: CreatePullRequestInput!) { createPullRequest(input: $input) { pullRequest { id url number } } }",
        json!({"input": {"baseRefName": "main", "body": "B", "draft": false, "headRefName": "topic", "maintainerCanModify": true, "repositoryId": rid, "title": "Topic"}}),
    )
    .await;
    let pr = &d["createPullRequest"]["pullRequest"];
    let pr_id = pr["id"].as_str().unwrap().to_string();
    let number = pr["number"].as_i64().unwrap();
    assert!(pr_id.starts_with("PR_"));

    // Request a review (GHES path uses user node ids).
    let rev_id = data(&s, &reviewer, "{viewer{id}}", json!({})).await["viewer"]["id"].as_str().unwrap().to_string();
    data(
        &s,
        &u,
        "mutation PullRequestCreateRequestReviews($input: RequestReviewsInput!) { requestReviews(input: $input) { clientMutationId } }",
        json!({"input": {"pullRequestId": pr_id, "union": true, "userIds": [rev_id]}}),
    )
    .await;

    // gh pr view: reviewRequests union with ...on Bot / ...on Team (User.name vs Team.name overlap).
    let view = "query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) { repository(owner: $owner, name: $repo) { pullRequest(number: $pr_number) {number,state,isDraft,mergeable,mergeStateStatus,additions,deletions,changedFiles,headRefName,baseRefName,isCrossRepository,headRepositoryOwner{id,login,...on User{name}},commits(first:100){totalCount,nodes{commit{oid,messageHeadline}}},files(first:100){nodes{path,additions,changeType}},reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}},reviewDecision,latestReviews(first:100){nodes{author{login},state}},statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state}}}}}}}} } }";
    let d = data(&s, &u, view, json!({"owner": u.login, "repo": repo, "pr_number": number})).await;
    let p = &d["repository"]["pullRequest"];
    assert_eq!(p["state"], "OPEN");
    assert_eq!(p["mergeable"], "MERGEABLE");
    assert_eq!(p["mergeStateStatus"], "CLEAN");
    assert_eq!(p["commits"]["totalCount"], 2);
    assert_eq!(p["commits"]["nodes"][1]["commit"]["messageHeadline"], "Add b");
    assert_eq!(p["files"]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(p["files"]["nodes"][0]["changeType"], "ADDED");
    assert_eq!(p["additions"], 2);
    assert_eq!(p["isCrossRepository"], false);
    assert_eq!(p["headRepositoryOwner"]["login"], u.login);
    assert_eq!(p["reviewRequests"]["nodes"][0]["requestedReviewer"]["login"], reviewer.login);
    assert_eq!(p["statusCheckRollup"]["nodes"][0]["commit"]["statusCheckRollup"], Value::Null);

    // The author cannot approve; the reviewer can.
    let review = "mutation PullRequestReviewAdd($input:AddPullRequestReviewInput!){addPullRequestReview(input:$input){clientMutationId}}";
    let v = gql(&s, &u, review, json!({"input": {"pullRequestId": pr_id, "event": "APPROVE", "body": ""}})).await;
    assert!(v["errors"].is_array());
    data(&s, &reviewer, review, json!({"input": {"pullRequestId": pr_id, "event": "APPROVE", "body": ""}})).await;
    let d = data(&s, &u, view, json!({"owner": u.login, "repo": repo, "pr_number": number})).await;
    assert_eq!(d["repository"]["pullRequest"]["reviewDecision"], "APPROVED");
    assert_eq!(d["repository"]["pullRequest"]["latestReviews"]["nodes"][0]["state"], "APPROVED");
    assert_eq!(d["repository"]["pullRequest"]["reviewRequests"]["nodes"], json!([]));

    // PullRequestForBranch (states: null) finds it by bare head name.
    let d = data(
        &s,
        &u,
        "query PullRequestForBranch($owner: String!, $repo: String!, $headRefName: String!, $states: [PullRequestState!]) { repository(owner: $owner, name: $repo) { pullRequests(headRefName: $headRefName, states: $states, first: 30, orderBy: { field: CREATED_AT, direction: DESC }) { nodes {number,headRefName} } defaultBranchRef { name } } }",
        json!({"owner": u.login, "repo": repo, "headRefName": "topic", "states": null}),
    )
    .await;
    assert_eq!(d["repository"]["pullRequests"]["nodes"][0]["number"], number);
    assert_eq!(d["repository"]["defaultBranchRef"]["name"], "main");

    // Squash merge.
    data(
        &s,
        &u,
        "mutation PullRequestMerge($input:MergePullRequestInput!){mergePullRequest(input: $input){clientMutationId}}",
        json!({"input": {"pullRequestId": pr_id, "mergeMethod": "SQUASH"}}),
    )
    .await;
    let d = data(
        &s,
        &u,
        "query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){state merged mergedBy{login} mergeCommit{oid messageHeadline}} defaultBranchRef{target{oid ...on Commit{messageHeadline}}}}}",
        json!({"o": u.login, "r": repo, "n": number}),
    )
    .await;
    let p = &d["repository"]["pullRequest"];
    assert_eq!(p["state"], "MERGED");
    assert_eq!(p["mergedBy"]["login"], u.login);
    assert_eq!(p["mergeCommit"]["oid"], d["repository"]["defaultBranchRef"]["target"]["oid"]);
    assert_eq!(d["repository"]["defaultBranchRef"]["target"]["messageHeadline"], format!("Topic (#{number})"));

    // Merging again is an error with a GitHub-style type.
    let v = gql(
        &s,
        &u,
        "mutation PullRequestMerge($input:MergePullRequestInput!){mergePullRequest(input: $input){clientMutationId}}",
        json!({"input": {"pullRequestId": pr_id, "mergeMethod": "MERGE"}}),
    )
    .await;
    assert!(v["errors"][0]["type"].is_string(), "{v}");
}

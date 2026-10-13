//! Projects (v2) over GraphQL: the queries and mutations `gh project` uses.
mod common;
use common::{TestServer, TestUser};
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
    assert_eq!(res.status().as_u16(), 200);
    res.json().await.unwrap()
}

async fn data(s: &TestServer, u: &TestUser, query: &str, vars: Value) -> Value {
    let v = gql(s, u, query, vars).await;
    assert!(v.get("errors").is_none(), "unexpected errors: {v}");
    v["data"].clone()
}

const PROJECT: &str = "number url title shortDescription public closed id readme items(first: $fi, after: null){totalCount} fields(first: $ff, after: null){totalCount} owner{__typename ... on User{login} ... on Organization{login}}";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_lifecycle() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let (st, issue) = s.call(Some(&u), "POST", &format!("/repos/{}/{repo}/issues", u.login), Some(json!({"title": "Tracked", "labels": ["bug"]}))).await;
    assert_eq!(st, 201);

    let d = data(&s, &u, "{viewer{id}}", json!({})).await;
    let owner_id = d["viewer"]["id"].as_str().unwrap().to_string();

    // create (with GitHub's default fields)
    let q = format!("mutation($input: CreateProjectV2Input!, $fi: Int!, $ff: Int!){{createProjectV2(input: $input){{projectV2{{{PROJECT}}}}}}}");
    let d = data(&s, &u, &q, json!({"input": {"ownerId": owner_id, "title": "Roadmap"}, "fi": 0, "ff": 0})).await;
    let p = &d["createProjectV2"]["projectV2"];
    assert_eq!(p["number"], 1);
    assert_eq!(p["title"], "Roadmap");
    assert_eq!(p["url"], format!("{}/users/{}/projects/1", s.base, u.login));
    assert_eq!(p["owner"], json!({"__typename": "User", "login": u.login}));
    assert_eq!(p["fields"]["totalCount"], 8);
    let pid = p["id"].as_str().unwrap().to_string();
    assert!(pid.starts_with("PVT_"));

    // private: invisible to others; owner lookups by number
    let v = gql(&s, &other, "query($l:String!){user(login:$l){projectV2(number:1){title}}}", json!({"l": u.login})).await;
    assert_eq!(v["errors"][0]["type"], "NOT_FOUND", "{v}");
    let d = data(&s, &u, "query($l:String!){user(login:$l){projectsV2(first:10){totalCount nodes{title}}}}", json!({"l": u.login})).await;
    assert_eq!(d["user"]["projectsV2"]["totalCount"], 1);
    let d = data(&s, &other, "query($l:String!){user(login:$l){projectsV2(first:10){totalCount}}}", json!({"l": u.login})).await;
    assert_eq!(d["user"]["projectsV2"]["totalCount"], 0);
    // ... and others can't modify
    let v = gql(&s, &other, "mutation($p:ID!){updateProjectV2(input:{projectId:$p,title:\"x\"}){projectV2{id}}}", json!({"p": pid})).await;
    assert!(v["errors"].is_array(), "{v}");

    // update + close
    let d = data(&s, &u, "mutation($p:ID!){updateProjectV2(input:{projectId:$p,shortDescription:\"Plans\",closed:true,public:true}){projectV2{shortDescription closed public closedAt}}}", json!({"p": pid})).await;
    let up = &d["updateProjectV2"]["projectV2"];
    assert_eq!((up["shortDescription"].as_str(), up["closed"].as_bool(), up["public"].as_bool()), (Some("Plans"), Some(true), Some(true)));
    assert!(up["closedAt"].is_string());

    // custom fields
    let d = data(
        &s,
        &u,
        "mutation($p:ID!){createProjectV2Field(input:{projectId:$p,dataType:SINGLE_SELECT,name:\"Priority\",singleSelectOptions:[{name:\"High\",color:GRAY,description:\"\"},{name:\"Low\",color:GRAY,description:\"\"}]}){projectV2Field{__typename ... on ProjectV2SingleSelectField{id name dataType options{id name}}}}}",
        json!({"p": pid}),
    )
    .await;
    let f = &d["createProjectV2Field"]["projectV2Field"];
    assert_eq!(f["dataType"], "SINGLE_SELECT");
    let field_id = f["id"].as_str().unwrap().to_string();
    let high = f["options"][0]["id"].as_str().unwrap().to_string();
    let d = data(&s, &u, "mutation($p:ID!){createProjectV2Field(input:{projectId:$p,dataType:ITERATION,name:\"Sprint\"}){projectV2Field{... on ProjectV2IterationField{dataType configuration{iterations{id title}}}}}}", json!({"p": pid})).await;
    assert_eq!(d["createProjectV2Field"]["projectV2Field"]["configuration"]["iterations"].as_array().unwrap().len(), 3);

    // items: an issue (via resource(url:)) and a draft
    let url = issue["html_url"].as_str().unwrap();
    let d = data(&s, &u, "query($u:URI!){resource(url:$u){__typename ... on Issue{id}}}", json!({"u": url})).await;
    assert_eq!(d["resource"]["__typename"], "Issue");
    let content_id = d["resource"]["id"].as_str().unwrap().to_string();
    let d = data(&s, &u, "mutation($p:ID!,$c:ID!){addProjectV2ItemById(input:{projectId:$p,contentId:$c}){item{id type content{__typename ... on Issue{title}}}}}", json!({"p": pid, "c": content_id})).await;
    let item = &d["addProjectV2ItemById"]["item"];
    assert_eq!(item["type"], "ISSUE");
    let issue_item = item["id"].as_str().unwrap().to_string();
    let d = data(&s, &u, "mutation($p:ID!){addProjectV2DraftIssue(input:{projectId:$p,title:\"Draft\",body:\"b\"}){projectItem{id content{... on DraftIssue{id title body}}}}}", json!({"p": pid})).await;
    let draft = &d["addProjectV2DraftIssue"]["projectItem"];
    let draft_item = draft["id"].as_str().unwrap().to_string();
    let draft_id = draft["content"]["id"].as_str().unwrap().to_string();
    assert!(draft_id.starts_with("DI_"));
    let d = data(&s, &u, "mutation($d:ID!){updateProjectV2DraftIssue(input:{draftIssueId:$d,title:\"Draft 2\"}){draftIssue{title body}}}", json!({"d": draft_id})).await;
    assert_eq!(d["updateProjectV2DraftIssue"]["draftIssue"], json!({"title": "Draft 2", "body": "b"}));

    // field values: set a single select, read it back with the built-in values
    let d = data(
        &s,
        &u,
        "mutation($p:ID!,$i:ID!,$f:ID!,$o:String!){updateProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f,value:{singleSelectOptionId:$o}}){projectV2Item{id}}}",
        json!({"p": pid, "i": issue_item, "f": field_id, "o": high}),
    )
    .await;
    assert_eq!(d["updateProjectV2ItemFieldValue"]["projectV2Item"]["id"], issue_item);
    let d = data(
        &s,
        &u,
        "query($l:String!){user(login:$l){projectV2(number:1){items(first:10){totalCount nodes{id fieldValues(first:100){nodes{__typename ... on ProjectV2ItemFieldSingleSelectValue{name field{... on ProjectV2SingleSelectField{name}}} ... on ProjectV2ItemFieldTextValue{text} ... on ProjectV2ItemFieldLabelValue{labels(first:10){nodes{name}}}}}}}}}}",
        json!({"l": u.login}),
    )
    .await;
    let items = &d["user"]["projectV2"]["items"];
    assert_eq!(items["totalCount"], 2);
    let values = items["nodes"][0]["fieldValues"]["nodes"].as_array().unwrap();
    assert!(values.iter().any(|v| v["text"] == "Tracked"), "{values:?}");
    assert!(values.iter().any(|v| v["name"] == "High" && v["field"]["name"] == "Priority"), "{values:?}");
    assert!(values.iter().any(|v| v["labels"]["nodes"][0]["name"] == "bug"), "{values:?}");
    // the issue knows its project items (gh item-edit --url)
    let d = data(&s, &u, "query($u:URI!){resource(url:$u){... on Issue{projectItems(first:10){nodes{id project{id}}}}}}", json!({"u": url})).await;
    assert_eq!(d["resource"]["projectItems"]["nodes"][0]["id"], issue_item);
    // items query filter
    let d = data(&s, &u, "query($l:String!){user(login:$l){projectV2(number:1){items(first:10,query:\"priority:high\"){totalCount}}}}", json!({"l": u.login})).await;
    assert_eq!(d["user"]["projectV2"]["items"]["totalCount"], 1);

    // archive, delete item, delete field
    let d = data(&s, &u, "mutation($p:ID!,$i:ID!){archiveProjectV2Item(input:{projectId:$p,itemId:$i}){item{isArchived}}}", json!({"p": pid, "i": draft_item})).await;
    assert_eq!(d["archiveProjectV2Item"]["item"]["isArchived"], true);
    let d = data(&s, &u, "mutation($p:ID!,$i:ID!){deleteProjectV2Item(input:{projectId:$p,itemId:$i}){deletedItemId}}", json!({"p": pid, "i": draft_item})).await;
    assert_eq!(d["deleteProjectV2Item"]["deletedItemId"], draft_item);
    data(&s, &u, "mutation($f:ID!){deleteProjectV2Field(input:{fieldId:$f}){projectV2Field{... on ProjectV2SingleSelectField{name}}}}", json!({"f": field_id})).await;
    let d = data(&s, &u, "query($l:String!){user(login:$l){projectV2(number:1){fields(first:50){totalCount}}}}", json!({"l": u.login})).await;
    assert_eq!(d["user"]["projectV2"]["fields"]["totalCount"], 9);

    // link to a repository
    let d = data(&s, &u, "query($o:String!,$n:String!){repository(owner:$o,name:$n){id}}", json!({"o": u.login, "n": repo})).await;
    let rid = d["repository"]["id"].as_str().unwrap().to_string();
    data(&s, &u, "mutation($p:ID!,$r:ID!){linkProjectV2ToRepository(input:{projectId:$p,repositoryId:$r}){repository{id}}}", json!({"p": pid, "r": rid})).await;
    let d = data(&s, &u, "query($o:String!,$n:String!){repository(owner:$o,name:$n){projectsV2(first:5){nodes{title}}}}", json!({"o": u.login, "n": repo})).await;
    assert_eq!(d["repository"]["projectsV2"]["nodes"][0]["title"], "Roadmap");

    // copy and delete
    let q = format!("mutation($input: CopyProjectV2Input!, $fi: Int!, $ff: Int!){{copyProjectV2(input: $input){{projectV2{{{PROJECT}}}}}}}");
    let d = data(&s, &u, &q, json!({"input": {"projectId": pid, "ownerId": owner_id, "title": "Copy", "includeDraftIssues": true}, "fi": 10, "ff": 50})).await;
    let c = &d["copyProjectV2"]["projectV2"];
    assert_eq!((c["number"].as_i64(), c["items"]["totalCount"].as_i64(), c["fields"]["totalCount"].as_i64()), (Some(2), Some(0), Some(9)));
    let d = data(&s, &u, "mutation($p:ID!){deleteProjectV2(input:{projectId:$p}){projectV2{title}}}", json!({"p": pid})).await;
    assert_eq!(d["deleteProjectV2"]["projectV2"]["title"], "Roadmap");
    let v = gql(&s, &u, "query($l:String!){user(login:$l){projectV2(number:1){title}}}", json!({"l": u.login})).await;
    assert_eq!(v["errors"][0]["message"], "Could not resolve to a ProjectV2 with the number 1.");

    // templates are for organization projects only
    let v = gql(&s, &u, "mutation($p:ID!){markProjectV2AsTemplate(input:{projectId:$p}){projectV2{id}}}", json!({"p": c["id"]})).await;
    assert!(v["errors"].is_array(), "{v}");
}

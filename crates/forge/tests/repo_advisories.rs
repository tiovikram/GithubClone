//! Repository security advisories and private vulnerability reporting (used by the web UI's
//! Security tab): drafts, credits, CVE requests, publishing into the global advisory database,
//! visibility, and private reports.
mod common;

use common::TestServer;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advisories_draft_publish_and_reports() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let rp = format!("/repos/{}/{repo}", owner.login);
    let base = format!("{rp}/security-advisories");

    // Validation: summary, description and vulnerabilities are required; severity XOR cvss.
    let (st, _) = s.call(Some(&owner), "POST", &base, Some(json!({"summary": "x"}))).await;
    assert_eq!(st, 422);
    let vulns = json!([{"package": {"ecosystem": "npm", "name": "left-pad-e2e"}, "vulnerable_version_range": "< 1.3.0", "patched_versions": "1.3.0"}]);
    let (st, _) = s
        .call(Some(&owner), "POST", &base, Some(json!({"summary": "x", "description": "d", "vulnerabilities": vulns, "severity": "high", "cvss_vector_string": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"})))
        .await;
    assert_eq!(st, 422);
    // Non-admins cannot create.
    let (st, _) = s.call(Some(&other), "POST", &base, Some(json!({"summary": "x", "description": "d", "vulnerabilities": vulns}))).await;
    assert_eq!(st, 403);

    let (st, a) = s
        .call(
            Some(&owner),
            "POST",
            &base,
            Some(json!({
                "summary": "Prototype pollution in left-pad-e2e",
                "description": "Bad things happen.",
                "vulnerabilities": vulns,
                "cvss_vector_string": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H",
                "cwe_ids": ["CWE-1321"],
                "credits": [{"login": other.login, "type": "finder"}, {"login": owner.login, "type": "remediation_developer"}],
            })),
        )
        .await;
    assert_eq!(st, 201, "{a}");
    let ghsa = a["ghsa_id"].as_str().unwrap().to_string();
    assert!(ghsa.starts_with("GHSA-"));
    assert_eq!(a["state"], "draft");
    assert_eq!(a["severity"], "critical");
    assert_eq!(a["cvss"]["score"], 9.8);
    assert_eq!(a["cwes"][0]["cwe_id"], "CWE-1321");
    assert_eq!(a["author"]["login"], owner.login);
    let credits = a["credits_detailed"].as_array().unwrap();
    assert_eq!(credits.len(), 2);
    assert!(credits.iter().any(|c| c["user"]["login"] == other.login && c["state"] == "pending" && c["type"] == "finder"));
    assert!(credits.iter().any(|c| c["user"]["login"] == owner.login && c["state"] == "accepted"));

    // Drafts are private to the maintainers.
    assert_eq!(s.call(Some(&other), "GET", &format!("{base}/{ghsa}"), None).await.0, 404);
    let (_, l) = s.call(Some(&other), "GET", &base, None).await;
    assert_eq!(l.as_array().unwrap().len(), 0);
    let (_, l) = s.call(Some(&owner), "GET", &format!("{base}?state=draft"), None).await;
    assert_eq!(l.as_array().unwrap().len(), 1);

    // Edit, request a CVE (recorded once), then publish.
    let (st, e) = s.call(Some(&owner), "PATCH", &format!("{base}/{ghsa}"), Some(json!({"summary": "Prototype pollution in left-pad-e2e (edited)", "severity": "high"}))).await;
    assert_eq!(st, 200, "{e}");
    assert_eq!(e["severity"], "high");
    assert!(e["cvss"].is_null());
    assert_eq!(s.call(Some(&owner), "POST", &format!("{base}/{ghsa}/cve"), None).await.0, 202);
    assert_eq!(s.call(Some(&owner), "POST", &format!("{base}/{ghsa}/cve"), None).await.0, 422);
    assert_eq!(s.call(Some(&owner), "PATCH", &format!("{base}/{ghsa}"), Some(json!({"state": "withdrawn"}))).await.0, 422);
    let (st, p) = s.call(Some(&owner), "PATCH", &format!("{base}/{ghsa}"), Some(json!({"state": "published"}))).await;
    assert_eq!(st, 200, "{p}");
    assert_eq!(p["state"], "published");
    assert_eq!(p["publisher"]["login"], owner.login);
    assert!(p["published_at"].is_string());
    // Published: visible to everyone, and in the global advisory database.
    assert_eq!(s.call(Some(&other), "GET", &format!("{base}/{ghsa}"), None).await.0, 200);
    let (st, g) = s.call(None, "GET", &format!("/advisories/{ghsa}"), None).await;
    assert_eq!(st, 200, "{g}");
    assert_eq!(g["severity"], "high");
    assert_eq!(g["vulnerabilities"][0]["package"]["name"], "left-pad-e2e");
    assert_eq!(g["vulnerabilities"][0]["vulnerable_version_range"], "< 1.3.0");
    // No going back to draft.
    assert_eq!(s.call(Some(&owner), "PATCH", &format!("{base}/{ghsa}"), Some(json!({"state": "draft"}))).await.0, 422);

    // Private vulnerability reporting: off by default.
    let pvr = format!("{rp}/private-vulnerability-reporting");
    assert_eq!(s.call(Some(&other), "GET", &pvr, None).await.1["enabled"], false);
    let report = json!({"summary": "XSS in the README renderer", "description": "Steps to reproduce...", "severity": "medium",
                        "vulnerabilities": [{"package": {"ecosystem": "npm", "name": "left-pad-e2e"}}]});
    assert_eq!(s.call(Some(&other), "POST", &format!("{base}/reports"), Some(report.clone())).await.0, 403);
    assert_eq!(s.call(Some(&other), "PUT", &pvr, None).await.0, 403);
    assert_eq!(s.call(Some(&owner), "PUT", &pvr, None).await.0, 204);
    assert_eq!(s.call(Some(&other), "GET", &pvr, None).await.1["enabled"], true);
    let (st, r) = s.call(Some(&other), "POST", &format!("{base}/reports"), Some(report)).await;
    assert_eq!(st, 201, "{r}");
    assert_eq!(r["state"], "triage");
    assert_eq!(r["submission"]["accepted"], false);
    let rg = r["ghsa_id"].as_str().unwrap().to_string();
    // The reporter sees their report; the maintainer accepts it (→ draft).
    assert_eq!(s.call(Some(&other), "GET", &format!("{base}/{rg}"), None).await.0, 200);
    assert_eq!(s.call(Some(&other), "PATCH", &format!("{base}/{rg}"), Some(json!({"state": "draft"}))).await.0, 403);
    let (st, acc) = s.call(Some(&owner), "PATCH", &format!("{base}/{rg}"), Some(json!({"state": "draft"}))).await;
    assert_eq!(st, 200);
    assert_eq!(acc["submission"]["accepted"], true);
    let (st, c) = s.call(Some(&owner), "PATCH", &format!("{base}/{rg}"), Some(json!({"state": "closed"}))).await;
    assert_eq!(st, 200);
    assert!(c["closed_at"].is_string());
    assert_eq!(s.call(Some(&owner), "DELETE", &pvr, None).await.0, 204);
    assert_eq!(s.call(Some(&owner), "GET", &pvr, None).await.1["enabled"], false);
}

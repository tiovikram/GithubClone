//! GraphQL global node ids: opaque base64url of "<Type>:<id>", with GitHub-like prefixes.
use base64::Engine;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

fn prefix(kind: &str) -> &'static str {
    match kind {
        "User" => "U_",
        "Organization" => "O_",
        "Bot" => "BOT_",
        "Repository" => "R_",
        "Issue" => "I_",
        "PullRequest" => "PR_",
        "IssueComment" => "IC_",
        "Label" => "LA_",
        "Milestone" => "MI_",
        "PullRequestReview" => "PRR_",
        "PullRequestReviewComment" => "PRRC_",
        "PullRequestReviewThread" => "PRRT_",
        "Release" => "RE_",
        "ReleaseAsset" => "RA_",
        "Commit" => "C_",
        "Ref" => "REF_",
        "ProjectV2" => "PVT_",
        "ProjectV2Item" => "PVTI_",
        "ProjectV2Field" => "PVTF_",
        "ProjectV2SingleSelectField" => "PVTSSF_",
        "ProjectV2IterationField" => "PVTIF_",
        "DraftIssue" => "DI_",
        _ => "N_",
    }
}

pub fn encode(kind: &str, id: i64) -> String {
    format!("{}{}", prefix(kind), B64.encode(format!("{kind}:{id}")))
}

/// Encode an id whose key is not an integer (e.g. a commit sha or ref name).
pub fn encode_str(kind: &str, key: &str) -> String {
    format!("{}{}", prefix(kind), B64.encode(format!("{kind}:{key}")))
}

/// Returns (kind, key) for a node id produced by `encode`/`encode_str`.
pub fn decode(node_id: &str) -> Option<(String, String)> {
    let (_, rest) = node_id.split_once('_')?;
    let raw = B64.decode(rest).ok()?;
    let s = String::from_utf8(raw).ok()?;
    let (k, v) = s.split_once(':')?;
    Some((k.to_string(), v.to_string()))
}

pub fn decode_int(node_id: &str, expected_kind: &str) -> Option<i64> {
    let (k, v) = decode(node_id)?;
    (k == expected_kind).then(|| v.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let id = super::encode("Repository", 42);
        assert!(id.starts_with("R_"));
        assert_eq!(super::decode_int(&id, "Repository"), Some(42));
        assert_eq!(super::decode_int(&id, "Issue"), None);
        let c = super::encode_str("Ref", "refs/heads/a_b");
        assert_eq!(super::decode(&c).unwrap().1, "refs/heads/a_b");
    }
}

//! Object creation helpers for the git data API (blobs, trees, commits, tags). Run these inside
//! `GitStore::write`. Objects that no ref reaches yet are not picked up by `git repack -d`, so
//! callers must finish with `pack_loose` to get them uploaded with the write.
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use anyhow::{bail, Context};
use tokio::io::AsyncWriteExt;

use super::cmd::GitCmd;
use super::read::{self, Signature};

/// Writes a blob, returning its sha.
pub async fn write_blob(git: &GitCmd, repo: &Path, data: &[u8]) -> anyhow::Result<String> {
    let out = git.run_input(repo, &["hash-object", "-w", "--stdin"], data).await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Writes an object of `kind` from its raw body (validated by git).
pub async fn write_object(git: &GitCmd, repo: &Path, kind: &str, data: &[u8]) -> anyhow::Result<String> {
    let out = git.run_input(repo, &["hash-object", "-t", kind, "-w", "--stdin"], data).await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Packs every loose object in the repository into a new pack (so `GitStore::write` uploads
/// objects that are not reachable from any ref yet).
pub async fn pack_loose(git: &GitCmd, repo: &Path) -> anyhow::Result<()> {
    let objects = repo.join("objects");
    let mut ids = Vec::new();
    let mut rd = tokio::fs::read_dir(&objects).await?;
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().to_string();
        if name.len() != 2 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let mut sub = tokio::fs::read_dir(e.path()).await?;
        while let Some(f) = sub.next_entry().await? {
            let rest = f.file_name().to_string_lossy().to_string();
            if rest.len() == 38 && rest.bytes().all(|b| b.is_ascii_hexdigit()) {
                ids.push(format!("{name}{rest}"));
            }
        }
    }
    if ids.is_empty() {
        return Ok(());
    }
    let input: Vec<u8> = ids.iter().flat_map(|i| format!("{i}\n").into_bytes()).collect();
    git.run_input(repo, &["pack-objects", "-q", "objects/pack/pack"], &input).await?;
    git.run(repo, &["prune-packed"]).await?;
    Ok(())
}

/// A change for `build_tree`. `sha = None` and `content = None` deletes the path.
#[derive(Debug, Clone)]
pub struct TreeChange {
    pub path: String,
    pub mode: String,
    pub kind: String,
    pub sha: Option<String>,
    pub content: Option<Vec<u8>>,
}

/// Error for user-caused failures (bad sha, bad mode, ...), mapped to 422 by the API.
#[derive(Debug)]
pub struct InvalidInput(pub String);

impl std::fmt::Display for InvalidInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for InvalidInput {}

fn invalid(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(InvalidInput(msg.into()))
}

#[derive(Debug, Clone)]
struct Ent {
    mode: String,
    kind: String,
    sha: String,
    /// Index of the loaded subtree node (trees only).
    child: Option<usize>,
}

#[derive(Debug, Default)]
struct Node {
    entries: BTreeMap<String, Ent>,
    dirty: bool,
    parent: Option<(usize, String)>,
}

async fn load_tree(git: &GitCmd, repo: &Path, sha: &str) -> anyhow::Result<BTreeMap<String, Ent>> {
    let obj = read::cat_object(git, repo, sha).await?.ok_or_else(|| invalid(format!("tree {sha} does not exist")))?;
    if obj.kind != "tree" {
        return Err(invalid(format!("{sha} is not a tree")));
    }
    Ok(read::parse_tree_object(&obj.data)
        .into_iter()
        .map(|(mode, name, sha)| {
            let kind = read::kind_for_mode(&mode).to_string();
            (name, Ent { mode, kind, sha, child: None })
        })
        .collect())
}

/// Creates a tree from `base` (a tree sha, or empty) with `changes` applied (paths may be
/// nested). Returns the new root tree sha. Unchanged subtrees are reused.
pub async fn build_tree(git: &GitCmd, repo: &Path, base: Option<&str>, changes: Vec<TreeChange>) -> anyhow::Result<String> {
    let root_entries = match base {
        Some(b) => load_tree(git, repo, b).await?,
        None => BTreeMap::new(),
    };
    let mut nodes: Vec<Node> = vec![Node { entries: root_entries, dirty: true, parent: None }];
    for ch in changes {
        let comps: Vec<&str> = ch.path.split('/').collect();
        // `mktree -z` does not validate names: reject NUL/control characters (which would
        // inject extra entries) and `.git` in any case (a repo-in-repo on case-insensitive
        // filesystems).
        if comps.iter().any(|c| c.is_empty() || *c == "." || *c == ".." || c.eq_ignore_ascii_case(".git") || c.chars().any(char::is_control)) {
            return Err(invalid(format!("tree.path {} is invalid", ch.path)));
        }
        // Resolve the leaf's sha (write blob content if given).
        let leaf_sha = match (&ch.sha, &ch.content) {
            (_, Some(content)) => {
                if ch.kind != "blob" {
                    return Err(invalid("tree.content can only be used with blobs"));
                }
                Some(write_blob(git, repo, content).await?)
            }
            (Some(s), None) => {
                if !read::is_sha(s) {
                    return Err(invalid(format!("tree.sha {s} is not a valid {}", ch.kind)));
                }
                Some(s.to_lowercase())
            }
            (None, None) => None,
        };
        let expected = read::kind_for_mode(&ch.mode);
        let valid_mode = matches!(ch.mode.as_str(), "100644" | "100755" | "040000" | "160000" | "120000");
        if leaf_sha.is_some() && (!valid_mode || expected != ch.kind) {
            return Err(invalid(format!("tree.mode {} is invalid for type {}", ch.mode, ch.kind)));
        }
        // Walk/create intermediate directories.
        let mut cur = 0usize;
        let mut missing_parent = false;
        for comp in &comps[..comps.len() - 1] {
            let existing = nodes[cur].entries.get(*comp).cloned();
            let next = match existing {
                Some(Ent { kind, child: Some(c), .. }) if kind == "tree" => c,
                Some(Ent { kind, sha, mode, .. }) if kind == "tree" => {
                    let entries = load_tree(git, repo, &sha).await?;
                    nodes.push(Node { entries, dirty: false, parent: Some((cur, comp.to_string())) });
                    let idx = nodes.len() - 1;
                    nodes[cur].entries.insert(comp.to_string(), Ent { mode, kind, sha, child: Some(idx) });
                    idx
                }
                _ => {
                    if leaf_sha.is_none() {
                        missing_parent = true;
                        break;
                    }
                    nodes.push(Node { entries: BTreeMap::new(), dirty: true, parent: Some((cur, comp.to_string())) });
                    let idx = nodes.len() - 1;
                    nodes[cur].entries.insert(
                        comp.to_string(),
                        Ent { mode: "040000".into(), kind: "tree".into(), sha: String::new(), child: Some(idx) },
                    );
                    idx
                }
            };
            cur = next;
        }
        if missing_parent {
            continue; // deleting something under a path that does not exist: no-op
        }
        let name = comps[comps.len() - 1].to_string();
        match leaf_sha {
            Some(sha) => {
                nodes[cur].entries.insert(name, Ent { mode: ch.mode.clone(), kind: ch.kind.clone(), sha, child: None });
            }
            None => {
                nodes[cur].entries.remove(&name);
            }
        }
        // Mark the path to the root dirty.
        let mut n = Some(cur);
        while let Some(i) = n {
            nodes[i].dirty = true;
            n = nodes[i].parent.as_ref().map(|(p, _)| *p);
        }
    }
    // Children always have larger indices than their parents: write in reverse order.
    for i in (0..nodes.len()).rev() {
        if !nodes[i].dirty {
            continue;
        }
        let empty = nodes[i].entries.is_empty();
        let sha = if empty && i != 0 {
            None
        } else {
            let mut input = Vec::new();
            for (name, e) in &nodes[i].entries {
                input.extend_from_slice(format!("{} {} {}\t{}\0", e.mode, e.kind, e.sha, name).as_bytes());
            }
            Some(mktree(git, repo, &input).await?)
        };
        if let Some((p, name)) = nodes[i].parent.clone() {
            match sha {
                Some(s) => {
                    if let Some(e) = nodes[p].entries.get_mut(&name) {
                        e.sha = s;
                    }
                }
                None => {
                    nodes[p].entries.remove(&name);
                }
            }
        } else {
            return sha.context("root tree");
        }
    }
    bail!("tree build failed")
}

async fn mktree(git: &GitCmd, repo: &Path, input: &[u8]) -> anyhow::Result<String> {
    let mut c = git.command(repo);
    c.args(["mktree", "-z"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn()?;
    let mut stdin = child.stdin.take().context("stdin")?;
    let data = input.to_vec();
    let w = tokio::spawn(async move {
        let _ = stdin.write_all(&data).await;
    });
    let out = child.wait_with_output().await?;
    let _ = w.await;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        return Err(invalid(format!("GitRPC::BadObjectState: {}", err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Writes a commit object with explicit author/committer (dates and timezones preserved).
pub async fn write_commit(
    git: &GitCmd,
    repo: &Path,
    tree: &str,
    parents: &[String],
    author: &Signature,
    committer: &Signature,
    message: &str,
    signature: Option<&str>,
) -> anyhow::Result<String> {
    let mut body = format!("tree {tree}\n");
    for p in parents {
        body.push_str(&format!("parent {p}\n"));
    }
    body.push_str(&format!("author {}\ncommitter {}\n", author.to_line(), committer.to_line()));
    if let Some(sig) = signature.filter(|s| !s.trim().is_empty()) {
        let sig = sig.trim_end_matches('\n').replace('\n', "\n ");
        body.push_str(&format!("gpgsig {sig}\n"));
    }
    body.push('\n');
    body.push_str(message);
    write_object(git, repo, "commit", body.as_bytes()).await
}

/// Writes an annotated tag object via `git mktag` (validates the target).
pub async fn write_tag(
    git: &GitCmd,
    repo: &Path,
    object: &str,
    kind: &str,
    tag: &str,
    tagger: &Signature,
    message: &str,
) -> anyhow::Result<String> {
    let mut msg = message.to_string();
    if !msg.ends_with('\n') {
        msg.push('\n');
    }
    let body = format!("object {object}\ntype {kind}\ntag {tag}\ntagger {}\n\n{msg}", tagger.to_line());
    match git.run_input(repo, &["mktag"], body.as_bytes()).await {
        Ok(out) => Ok(String::from_utf8_lossy(&out).trim().to_string()),
        Err(e) => Err(invalid(format!("{e}"))),
    }
}

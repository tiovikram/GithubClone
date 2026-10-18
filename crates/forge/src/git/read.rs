//! Read helpers over a bare repository: batched object reads (`cat-file --batch`), tree
//! listings, commit/tag parsing, history queries and diffs. All functions take the synced
//! bare-repo path (`state.git.sync(repo_id)`) and never mutate it.
use std::path::Path;
use std::process::Stdio;

use anyhow::{bail, Context};
use chrono::{DateTime, TimeZone, Utc};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use super::cmd::GitCmd;

pub const ZERO_SHA: &str = "0000000000000000000000000000000000000000";
/// The well-known empty tree (git resolves it without it being stored).
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// A raw git object.
#[derive(Debug, Clone)]
pub struct RawObject {
    pub sha: String,
    pub kind: String,
    pub data: Vec<u8>,
}

pub fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A revision safe to pass to git (never parsed as an option, no control characters).
pub fn safe_rev(rev: &str) -> bool {
    !rev.is_empty() && !rev.starts_with('-') && !rev.chars().any(|c| c.is_control()) && !rev.contains("..")
}

/// Reads many objects with a single `git cat-file --batch` process. `revs` may be any object
/// expression (`<sha>`, `<commit>:<path>`); missing ones yield `None`.
pub async fn cat_objects(git: &GitCmd, repo: &Path, revs: &[String]) -> anyhow::Result<Vec<Option<RawObject>>> {
    if revs.is_empty() {
        return Ok(Vec::new());
    }
    if revs.iter().any(|r| r.contains('\n') || r.is_empty()) {
        bail!("invalid object name");
    }
    let mut c = git.command(repo);
    c.args(["cat-file", "--batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn()?;
    let mut stdin = child.stdin.take().context("stdin")?;
    let input: Vec<u8> = revs.iter().flat_map(|r| format!("{r}\n").into_bytes()).collect();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
    });
    let mut out = BufReader::new(child.stdout.take().context("stdout")?);
    let mut res = Vec::with_capacity(revs.len());
    let mut line = String::new();
    for _ in revs {
        line.clear();
        if out.read_line(&mut line).await? == 0 {
            bail!("cat-file ended early");
        }
        let header = line.trim_end_matches('\n');
        let parts: Vec<&str> = header.rsplitn(3, ' ').collect();
        // "<sha> <type> <size>" or "<rev> missing" / "<rev> ambiguous"
        let size = match parts.first().and_then(|s| s.parse::<usize>().ok()) {
            Some(n) if parts.len() == 3 && is_sha(parts[2]) => n,
            _ => {
                res.push(None);
                continue;
            }
        };
        let mut data = vec![0u8; size + 1];
        out.read_exact(&mut data).await?;
        data.pop();
        res.push(Some(RawObject { sha: parts[2].to_string(), kind: parts[1].to_string(), data }));
    }
    let _ = writer.await;
    let _ = child.wait().await;
    Ok(res)
}

pub async fn cat_object(git: &GitCmd, repo: &Path, rev: &str) -> anyhow::Result<Option<RawObject>> {
    Ok(cat_objects(git, repo, &[rev.to_string()]).await?.pop().flatten())
}

/// (type, size) for each rev using `cat-file --batch-check`.
pub async fn object_info(git: &GitCmd, repo: &Path, revs: &[String]) -> anyhow::Result<Vec<Option<(String, String, u64)>>> {
    if revs.is_empty() {
        return Ok(Vec::new());
    }
    if revs.iter().any(|r| r.contains('\n') || r.is_empty()) {
        bail!("invalid object name");
    }
    let input: Vec<u8> = revs.iter().flat_map(|r| format!("{r}\n").into_bytes()).collect();
    let out = git.run_input(repo, &["cat-file", "--batch-check"], &input).await?;
    let text = String::from_utf8_lossy(&out);
    Ok(text
        .lines()
        .map(|l| {
            let p: Vec<&str> = l.rsplitn(3, ' ').collect();
            if p.len() == 3 && is_sha(p[2]) {
                p[0].parse().ok().map(|n| (p[2].to_string(), p[1].to_string(), n))
            } else {
                None
            }
        })
        .collect())
}

/// One entry of a tree listing.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub mode: String,
    pub kind: String,
    pub sha: String,
    /// Blob size (None for trees and submodules).
    pub size: Option<u64>,
    /// Path relative to the tree root.
    pub path: String,
}

impl TreeEntry {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// `git ls-tree -l -z` on `treeish`; `path` restricts to that exact entry (literal pathspec).
pub async fn ls_tree(git: &GitCmd, repo: &Path, treeish: &str, path: Option<&str>, recursive: bool) -> anyhow::Result<Vec<TreeEntry>> {
    let mut args = vec!["--literal-pathspecs", "ls-tree", "-l", "-z", "--full-tree"];
    if recursive {
        args.push("-r");
        args.push("-t");
    }
    args.push(treeish);
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let out = git.run(repo, &args).await?;
    Ok(parse_ls_tree(&out))
}

pub fn parse_ls_tree(out: &[u8]) -> Vec<TreeEntry> {
    out.split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|rec| {
            let tab = rec.iter().position(|b| *b == b'\t')?;
            let meta = String::from_utf8_lossy(&rec[..tab]).to_string();
            let path = String::from_utf8_lossy(&rec[tab + 1..]).to_string();
            let mut it = meta.split_whitespace();
            let mode = it.next()?.to_string();
            let kind = it.next()?.to_string();
            let sha = it.next()?.to_string();
            let size = it.next().and_then(|s| s.parse().ok());
            Some(TreeEntry { mode, kind, sha, size, path })
        })
        .collect()
}

/// Parses a raw tree object into (mode, name, sha) entries.
pub fn parse_tree_object(data: &[u8]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let Some(sp) = data[i..].iter().position(|b| *b == b' ') else { break };
        let mode = String::from_utf8_lossy(&data[i..i + sp]).to_string();
        let rest = i + sp + 1;
        let Some(nul) = data[rest..].iter().position(|b| *b == 0) else { break };
        let name = String::from_utf8_lossy(&data[rest..rest + nul]).to_string();
        let sha_start = rest + nul + 1;
        if sha_start + 20 > data.len() {
            break;
        }
        let sha = hex::encode(&data[sha_start..sha_start + 20]);
        // Trees store "40000" for directories; normalize to the 6-digit form GitHub shows.
        let mode = if mode.len() == 5 { format!("0{mode}") } else { mode };
        out.push((mode, name, sha));
        i = sha_start + 20;
    }
    out
}

pub fn kind_for_mode(mode: &str) -> &'static str {
    match mode {
        "040000" | "40000" => "tree",
        "160000" => "commit",
        _ => "blob",
    }
}

/// An author/committer/tagger line.
#[derive(Debug, Clone)]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub time: i64,
    /// e.g. "+0200"
    pub tz: String,
}

impl Signature {
    pub fn parse(s: &str) -> Signature {
        let (ident, rest) = match s.rfind('>') {
            Some(i) => (&s[..=i], s[i + 1..].trim()),
            None => (s, ""),
        };
        let (name, email) = match ident.rfind('<') {
            Some(i) => (ident[..i].trim().to_string(), ident[i + 1..].trim_end_matches('>').to_string()),
            None => (ident.trim().to_string(), String::new()),
        };
        let mut it = rest.split_whitespace();
        let time = it.next().and_then(|t| t.parse().ok()).unwrap_or(0);
        let tz = it.next().unwrap_or("+0000").to_string();
        Signature { name, email, time, tz }
    }
    pub fn date(&self) -> DateTime<Utc> {
        Utc.timestamp_opt(self.time, 0).single().unwrap_or_default()
    }
    /// `Name <email> <unix> <tz>` as stored in objects.
    pub fn to_line(&self) -> String {
        format!("{} <{}> {} {}", clean_ident(&self.name), clean_ident(&self.email), self.time, self.tz)
    }
}

fn clean_ident(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '<' | '>' | '\n' | '\r' | '\0')).collect::<String>().trim().to_string()
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub tree: String,
    pub parents: Vec<String>,
    pub author: Signature,
    pub committer: Signature,
    pub message: String,
    /// gpgsig header (armored signature), if signed.
    pub signature: Option<String>,
    /// The signed payload (commit without the gpgsig header), if signed.
    pub payload: Option<String>,
}

/// Splits an object into (headers, message), handling continuation lines.
fn split_headers(text: &str) -> (Vec<(String, String)>, String, String) {
    let (head, msg) = match text.find("\n\n") {
        Some(i) => (&text[..i], &text[i + 2..]),
        None => (text.trim_end_matches('\n'), ""),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut unsigned = String::new();
    let mut in_sig = false;
    for line in head.split('\n') {
        if let Some(cont) = line.strip_prefix(' ') {
            if let Some(last) = headers.last_mut() {
                last.1.push('\n');
                last.1.push_str(cont);
            }
            if !in_sig {
                unsigned.push_str(line);
                unsigned.push('\n');
            }
            continue;
        }
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        in_sig = k == "gpgsig" || k == "gpgsig-sha256";
        if !in_sig {
            unsigned.push_str(line);
            unsigned.push('\n');
        }
        headers.push((k.to_string(), v.to_string()));
    }
    unsigned.push('\n');
    unsigned.push_str(msg);
    (headers, msg.to_string(), unsigned)
}

pub fn parse_commit(sha: &str, data: &[u8]) -> CommitInfo {
    let text = String::from_utf8_lossy(data);
    let (headers, message, unsigned) = split_headers(&text);
    let mut c = CommitInfo {
        sha: sha.to_string(),
        tree: String::new(),
        parents: Vec::new(),
        author: Signature::parse(""),
        committer: Signature::parse(""),
        message,
        signature: None,
        payload: None,
    };
    for (k, v) in headers {
        match k.as_str() {
            "tree" => c.tree = v,
            "parent" => c.parents.push(v),
            "author" => c.author = Signature::parse(&v),
            "committer" => c.committer = Signature::parse(&v),
            "gpgsig" | "gpgsig-sha256" => {
                c.signature = Some(format!("{v}\n").replace("\n\n", "\n"));
                c.payload = Some(unsigned.clone());
            }
            _ => {}
        }
    }
    c
}

#[derive(Debug, Clone)]
pub struct TagInfo {
    pub sha: String,
    pub object: String,
    pub kind: String,
    pub tag: String,
    pub tagger: Option<Signature>,
    pub message: String,
    pub signature: Option<String>,
}

pub fn parse_tag(sha: &str, data: &[u8]) -> TagInfo {
    let text = String::from_utf8_lossy(data);
    let (headers, message, _) = split_headers(&text);
    let mut t = TagInfo {
        sha: sha.to_string(),
        object: String::new(),
        kind: String::new(),
        tag: String::new(),
        tagger: None,
        message: message.clone(),
        signature: None,
    };
    for (k, v) in headers {
        match k.as_str() {
            "object" => t.object = v,
            "type" => t.kind = v,
            "tag" => t.tag = v,
            "tagger" => t.tagger = Some(Signature::parse(&v)),
            _ => {}
        }
    }
    if let Some(i) = message.find("-----BEGIN PGP SIGNATURE-----").or_else(|| message.find("-----BEGIN SSH SIGNATURE-----")) {
        t.signature = Some(message[i..].to_string());
        t.message = message[..i].to_string();
    }
    t
}

/// Loads and parses commits by sha (one process). Missing or non-commit objects yield None.
pub async fn read_commits(git: &GitCmd, repo: &Path, shas: &[String]) -> anyhow::Result<Vec<Option<CommitInfo>>> {
    let objs = cat_objects(git, repo, shas).await?;
    Ok(objs
        .into_iter()
        .map(|o| o.filter(|o| o.kind == "commit").map(|o| parse_commit(&o.sha, &o.data)))
        .collect())
}

pub async fn read_commit(git: &GitCmd, repo: &Path, sha: &str) -> anyhow::Result<Option<CommitInfo>> {
    Ok(read_commits(git, repo, &[sha.to_string()]).await?.pop().flatten())
}

/// Resolves a revision (branch, tag, sha, `refs/...`) to a commit sha.
pub async fn resolve_commit(git: &GitCmd, repo: &Path, rev: &str) -> anyhow::Result<Option<String>> {
    if !safe_rev(rev) {
        return Ok(None);
    }
    let spec = format!("{rev}^{{commit}}");
    let (ok, out, _) = git.try_run(repo, &["rev-parse", "--verify", "--quiet", "--end-of-options", &spec]).await?;
    let s = String::from_utf8_lossy(&out).trim().to_string();
    Ok((ok && is_sha(&s)).then_some(s))
}

/// Resolves a revision to any object; returns (sha, type).
pub async fn resolve_object(git: &GitCmd, repo: &Path, rev: &str) -> anyhow::Result<Option<(String, String)>> {
    if !safe_rev(rev) {
        return Ok(None);
    }
    Ok(object_info(git, repo, &[rev.to_string()]).await?.pop().flatten().map(|(s, k, _)| (s, k)))
}

pub async fn is_ancestor(git: &GitCmd, repo: &Path, ancestor: &str, descendant: &str) -> anyhow::Result<bool> {
    let (ok, _, _) = git.try_run(repo, &["merge-base", "--is-ancestor", ancestor, descendant]).await?;
    Ok(ok)
}

pub async fn merge_base(git: &GitCmd, repo: &Path, a: &str, b: &str) -> anyhow::Result<Option<String>> {
    let (ok, out, _) = git.try_run(repo, &["merge-base", a, b]).await?;
    let s = String::from_utf8_lossy(&out).trim().to_string();
    Ok((ok && is_sha(&s)).then_some(s))
}

/// Commit history query (`git rev-list`).
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Revision range arguments, e.g. ["main"] or ["^base", "head"].
    pub revs: Vec<String>,
    pub path: Option<String>,
    /// Author patterns (OR-ed); matched as fixed strings against "Name <email>".
    pub authors: Vec<String>,
    pub committers: Vec<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub skip: usize,
    pub limit: Option<usize>,
    pub reverse: bool,
}

impl LogQuery {
    fn args(&self, count: bool) -> Vec<String> {
        let mut a: Vec<String> = vec!["--literal-pathspecs".into(), "rev-list".into()];
        if count {
            a.push("--count".into());
        } else {
            if self.skip > 0 {
                a.push(format!("--skip={}", self.skip));
            }
            if let Some(l) = self.limit {
                a.push(format!("--max-count={l}"));
            }
            if self.reverse {
                a.push("--reverse".into());
            }
        }
        if !self.authors.is_empty() || !self.committers.is_empty() {
            a.push("--fixed-strings".into());
            a.push("--regexp-ignore-case".into());
        }
        for x in &self.authors {
            a.push(format!("--author={x}"));
        }
        for x in &self.committers {
            a.push(format!("--committer={x}"));
        }
        if let Some(s) = &self.since {
            a.push(format!("--since={s}"));
        }
        if let Some(u) = &self.until {
            a.push(format!("--until={u}"));
        }
        a.push("--end-of-options".into());
        a.extend(self.revs.iter().cloned());
        a.push("--".into());
        if let Some(p) = &self.path {
            a.push(p.clone());
        }
        a
    }
}

pub async fn rev_list(git: &GitCmd, repo: &Path, q: &LogQuery) -> anyhow::Result<Vec<String>> {
    let args = q.args(false);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = git.run(repo, &refs).await?;
    Ok(String::from_utf8_lossy(&out).lines().filter(|l| is_sha(l)).map(str::to_string).collect())
}

pub async fn rev_count(git: &GitCmd, repo: &Path, q: &LogQuery) -> anyhow::Result<usize> {
    let args = q.args(true);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = git.run(repo, &refs).await?;
    Ok(String::from_utf8_lossy(&out).trim().parse().unwrap_or(0))
}

/// One changed file between two trees.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// GitHub status: added, removed, modified, renamed, copied, changed.
    pub status: &'static str,
    pub path: String,
    pub previous_path: Option<String>,
    pub old_sha: String,
    pub new_sha: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
    pub patch: Option<String>,
}

/// Maximum files returned for a diff (GitHub caps commit/compare files at 300).
pub const MAX_DIFF_FILES: usize = 300;
const MAX_PATCH_BYTES: usize = 1024 * 1024;

/// Diffs `from` (None = empty tree) against `to` (commit or tree ish). With `with_patch`, each
/// text file carries its unified diff hunks like GitHub's `patch` field.
pub async fn diff(git: &GitCmd, repo: &Path, from: Option<&str>, to: &str, with_patch: bool) -> anyhow::Result<Vec<FileChange>> {
    let from = from.unwrap_or(EMPTY_TREE);
    let raw = git.run(repo, &["diff-tree", "-r", "-z", "-M", "--raw", from, to]).await?;
    let mut files = parse_raw_diff(&raw);
    let num = git.run(repo, &["diff-tree", "-r", "-z", "-M", "--numstat", from, to]).await?;
    let stats = parse_numstat(&num);
    for (f, (a, d, bin)) in files.iter_mut().zip(stats.into_iter()) {
        f.additions = a;
        f.deletions = d;
        f.binary = bin;
    }
    if with_patch && !files.is_empty() {
        // Bounded (FORGE_MAX_DIFF_BYTES): beyond it, patches are omitted like GitHub does for
        // large diffs; the file list and stats above are complete.
        let (out, truncated) = crate::security::git_output_capped(
            git,
            repo,
            &["diff-tree", "-r", "-M", "-p", "--no-color", "--no-ext-diff", "--no-textconv", from, to],
            crate::security::max_diff_bytes(),
        )
        .await?;
        if truncated {
            return Ok(files);
        }
        let chunks = split_patch(&out);
        for (f, chunk) in files.iter_mut().zip(chunks.into_iter()) {
            if !f.binary {
                f.patch = chunk;
            }
        }
    }
    Ok(files)
}

fn parse_raw_diff(out: &[u8]) -> Vec<FileChange> {
    let toks: Vec<String> = out.split(|b| *b == 0).map(|t| String::from_utf8_lossy(t).to_string()).collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if !t.starts_with(':') {
            i += 1;
            continue;
        }
        let meta: Vec<&str> = t[1..].split(' ').collect();
        if meta.len() < 5 {
            i += 1;
            continue;
        }
        let (old_sha, new_sha, st) = (meta[2].to_string(), meta[3].to_string(), meta[4]);
        let letter = st.chars().next().unwrap_or('M');
        let (status, prev, path, adv) = match letter {
            'R' | 'C' => (
                if letter == 'R' { "renamed" } else { "copied" },
                toks.get(i + 1).cloned(),
                toks.get(i + 2).cloned().unwrap_or_default(),
                3,
            ),
            'A' => ("added", None, toks.get(i + 1).cloned().unwrap_or_default(), 2),
            'D' => ("removed", None, toks.get(i + 1).cloned().unwrap_or_default(), 2),
            'T' => ("changed", None, toks.get(i + 1).cloned().unwrap_or_default(), 2),
            _ => ("modified", None, toks.get(i + 1).cloned().unwrap_or_default(), 2),
        };
        files.push(FileChange {
            status,
            path,
            previous_path: prev,
            old_sha,
            new_sha,
            additions: 0,
            deletions: 0,
            binary: false,
            patch: None,
        });
        i += adv;
    }
    files
}

fn parse_numstat(out: &[u8]) -> Vec<(u64, u64, bool)> {
    let toks: Vec<String> = out.split(|b| *b == 0).map(|t| String::from_utf8_lossy(t).to_string()).collect();
    let mut res = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let parts: Vec<&str> = t.splitn(3, '\t').collect();
        if parts.len() < 3 {
            i += 1;
            continue;
        }
        let bin = parts[0] == "-";
        res.push((parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0), bin));
        // Renames: "a\td\t" followed by two NUL-separated paths.
        i += if parts[2].is_empty() { 3 } else { 1 };
    }
    res
}

/// Splits `diff-tree -p` output per file; each item is the hunk text (from the first `@@`).
fn split_patch(out: &[u8]) -> Vec<Option<String>> {
    let text = String::from_utf8_lossy(out);
    let mut chunks: Vec<Vec<&str>> = Vec::new();
    for line in text.split('\n') {
        if line.starts_with("diff --git ") {
            chunks.push(Vec::new());
        } else if let Some(c) = chunks.last_mut() {
            c.push(line);
        }
    }
    chunks
        .into_iter()
        .map(|lines| {
            let start = lines.iter().position(|l| l.starts_with("@@"))?;
            let mut body = lines[start..].join("\n");
            while body.ends_with('\n') {
                body.pop();
            }
            (body.len() <= MAX_PATCH_BYTES).then_some(body)
        })
        .collect()
}

/// Plain-text unified diff between two revisions (`Accept: application/vnd.github.diff`).
/// Fails with `security::DiffTooLarge` beyond `FORGE_MAX_DIFF_BYTES`.
pub async fn diff_text(git: &GitCmd, repo: &Path, from: Option<&str>, to: &str) -> anyhow::Result<Vec<u8>> {
    let from = from.unwrap_or(EMPTY_TREE);
    crate::security::git_diff_capped(git, repo, &["diff-tree", "-r", "-M", "-p", "--full-index", "--no-color", "--no-ext-diff", "--no-textconv", from, to])
        .await
}

/// mbox-formatted patches (`Accept: application/vnd.github.patch`) for `range` (e.g. `a..b`) or
/// a single commit (`-1 <sha>`).
pub async fn format_patch(git: &GitCmd, repo: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let mut a = vec!["format-patch", "--stdout", "--no-color", "--no-signature", "--full-index", "--binary"];
    a.extend_from_slice(args);
    crate::security::git_diff_capped(git, repo, &a).await
}

/// Parses an ISO-8601 timestamp into (unix seconds, "+hhmm").
pub fn parse_iso_date(s: &str) -> Option<(i64, String)> {
    let dt = DateTime::parse_from_rfc3339(s.trim()).ok().or_else(|| {
        chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%dT%H:%M:%S")
            .ok()
            .map(|n| n.and_utc().fixed_offset())
    })?;
    let off = dt.offset().local_minus_utc();
    let sign = if off < 0 { '-' } else { '+' };
    let off = off.abs();
    Some((dt.timestamp(), format!("{sign}{:02}{:02}", off / 3600, (off % 3600) / 60)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_roundtrip() {
        let s = Signature::parse("A U Thor <a@example.com> 1700000000 +0200");
        assert_eq!(s.name, "A U Thor");
        assert_eq!(s.email, "a@example.com");
        assert_eq!(s.time, 1700000000);
        assert_eq!(s.tz, "+0200");
        assert_eq!(s.to_line(), "A U Thor <a@example.com> 1700000000 +0200");
    }

    #[test]
    fn commit_parse() {
        let raw = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nparent aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nauthor A <a@x> 1 +0000\ncommitter B <b@x> 2 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n abc\n -----END PGP SIGNATURE-----\n\nhello\n";
        let c = parse_commit("x", raw);
        assert_eq!(c.parents.len(), 1);
        assert_eq!(c.message, "hello\n");
        assert!(c.signature.unwrap().contains("abc"));
        assert!(!c.payload.unwrap().contains("gpgsig"));
    }

    #[test]
    fn raw_and_numstat() {
        let raw = b":100644 100644 aaaa bbbb M\0a.txt\0:000000 100644 0000 cccc A\0b.txt\0:100644 100644 dddd dddd R100\0old.txt\0new.txt\0";
        let f = parse_raw_diff(raw);
        assert_eq!(f.len(), 3);
        assert_eq!(f[2].status, "renamed");
        assert_eq!(f[2].previous_path.as_deref(), Some("old.txt"));
        let n = parse_numstat(b"1\t2\ta.txt\x003\t0\tb.txt\x000\t0\t\x00old.txt\x00new.txt\x00");
        assert_eq!(n, vec![(1, 2, false), (3, 0, false), (0, 0, false)]);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(parse_iso_date("2024-01-01T00:00:00Z"), Some((1704067200, "+0000".into())));
        assert_eq!(parse_iso_date("2024-01-01T02:00:00+02:00"), Some((1704067200, "+0200".into())));
    }
}

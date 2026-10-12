//! Git plumbing for pull requests: analysis (merge base, counts, mergeability), merge commit
//! creation (merge / squash / rebase) in a bare repository, commit listing and diff parsing.
//! Everything here runs `git` on a bare repo path; callers decide whether that is inside a
//! `state.git.write` (anything that creates objects or moves refs) or after `state.git.sync`.
use std::path::Path;
use std::process::Stdio;

use anyhow::{bail, Context};
use chrono::{DateTime, Utc};
use tokio::io::AsyncWriteExt;

use crate::git::cmd::{GitCmd, Identity};

pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
pub const ZERO_SHA: &str = "0000000000000000000000000000000000000000";

/// What a pull request looks like in git for a given base/head pair.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub merge_base: Option<String>,
    pub commits: i64,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub mergeable: bool,
    pub rebaseable: bool,
    /// Tree of the clean three-way merge, when mergeable.
    pub merge_tree: Option<String>,
}

async fn out_str(git: &GitCmd, repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    git.run_str(repo, args).await
}

/// `git merge-base a b`, None for unrelated histories.
pub async fn merge_base(git: &GitCmd, repo: &Path, a: &str, b: &str) -> anyhow::Result<Option<String>> {
    let (ok, out, _) = git.try_run(repo, &["merge-base", a, b]).await?;
    Ok(ok.then(|| String::from_utf8_lossy(&out).trim().to_string()).filter(|s| !s.is_empty()))
}

pub async fn is_ancestor(git: &GitCmd, repo: &Path, a: &str, b: &str) -> anyhow::Result<bool> {
    let (ok, _, _) = git.try_run(repo, &["merge-base", "--is-ancestor", a, b]).await?;
    Ok(ok)
}

pub async fn has_commit(git: &GitCmd, repo: &Path, sha: &str) -> bool {
    git.try_run(repo, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).await.map(|r| r.0).unwrap_or(false)
}

pub async fn count(git: &GitCmd, repo: &Path, range: &str, merges_only: bool) -> anyhow::Result<i64> {
    let mut args = vec!["rev-list", "--count"];
    if merges_only {
        args.push("--merges");
    }
    args.push(range);
    Ok(out_str(git, repo, &args).await?.parse().unwrap_or(0))
}

/// (files, additions, deletions) of `git diff from to`.
pub async fn numstat(git: &GitCmd, repo: &Path, from: &str, to: &str) -> anyhow::Result<(i64, i64, i64)> {
    let out = git.run(repo, &["diff", "--no-ext-diff", "--numstat", "-z", "-M", from, to]).await?;
    let mut files = 0;
    let (mut add, mut del) = (0i64, 0i64);
    let mut fields = out.split(|b| *b == 0).peekable();
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(rec);
        let mut parts = s.splitn(3, '\t');
        let a = parts.next().unwrap_or("-");
        let d = parts.next().unwrap_or("-");
        let path = parts.next().unwrap_or("");
        if path.is_empty() {
            // rename: "<a>\t<d>\t\0old\0new\0"
            fields.next();
            fields.next();
        }
        files += 1;
        add += a.parse::<i64>().unwrap_or(0);
        del += d.parse::<i64>().unwrap_or(0);
    }
    Ok((files, add, del))
}

/// Three-way merge of `ours` and `theirs` without a worktree. Returns the merged tree, or None
/// on conflicts (or unrelated histories). `merge_base` forces a specific base (cherry-pick).
pub async fn merge_tree(git: &GitCmd, repo: &Path, ours: &str, theirs: &str, merge_base: Option<&str>) -> anyhow::Result<Option<String>> {
    let mb_arg = merge_base.map(|m| format!("--merge-base={m}"));
    let mut args = vec!["merge-tree", "--write-tree", "--no-messages"];
    if let Some(m) = &mb_arg {
        args.push(m);
    }
    args.push(ours);
    args.push(theirs);
    let (ok, out, err) = git.try_run(repo, &args).await?;
    if ok {
        let tree = String::from_utf8_lossy(&out).lines().next().unwrap_or("").trim().to_string();
        if tree.len() >= 40 {
            return Ok(Some(tree));
        }
        bail!("merge-tree produced no tree");
    }
    let err = String::from_utf8_lossy(&err);
    if err.trim().is_empty() || err.contains("unrelated histories") {
        // exit 1 with a tree on stdout = conflicts
        return Ok(None);
    }
    bail!("git merge-tree failed: {}", err.trim())
}

/// Analyzes base/head in a repo that has both objects. Creates objects (merge-tree) so this
/// belongs inside a write.
pub async fn analyze(git: &GitCmd, repo: &Path, base: &str, head: &str) -> anyhow::Result<Analysis> {
    let mb = merge_base(git, repo, base, head).await?;
    let commits = count(git, repo, &format!("{base}..{head}"), false).await?;
    let (changed_files, additions, deletions) = numstat(git, repo, mb.as_deref().unwrap_or(EMPTY_TREE), head).await?;
    let tree = if mb.is_some() { merge_tree(git, repo, base, head, None).await? } else { None };
    let mergeable = tree.is_some();
    let rebaseable = mergeable && count(git, repo, &format!("{base}..{head}"), true).await? == 0;
    Ok(Analysis { merge_base: mb, commits, additions, deletions, changed_files, mergeable, rebaseable, merge_tree: tree })
}

/// A git identity with a raw git date (`<unix> <tz>`), for preserving authorship on rebase.
#[derive(Debug, Clone)]
pub struct RawIdent {
    pub name: String,
    pub email: String,
    /// `"<unix seconds> <+hhmm>"`; empty = now.
    pub date: String,
}

impl From<&Identity> for RawIdent {
    fn from(i: &Identity) -> Self {
        RawIdent { name: i.name.clone(), email: i.email.clone(), date: i.when.map(|t| format!("{t} +0000")).unwrap_or_default() }
    }
}

/// `git commit-tree` with explicit author/committer (raw dates).
pub async fn commit_tree(git: &GitCmd, repo: &Path, tree: &str, parents: &[&str], message: &str, author: &RawIdent, committer: &RawIdent) -> anyhow::Result<String> {
    let mut c = git.command(repo);
    c.arg("commit-tree").arg(tree);
    for p in parents {
        c.arg("-p").arg(p);
    }
    c.env("GIT_AUTHOR_NAME", &author.name)
        .env("GIT_AUTHOR_EMAIL", &author.email)
        .env("GIT_COMMITTER_NAME", &committer.name)
        .env("GIT_COMMITTER_EMAIL", &committer.email);
    if !author.date.is_empty() {
        c.env("GIT_AUTHOR_DATE", &author.date);
    }
    if !committer.date.is_empty() {
        c.env("GIT_COMMITTER_DATE", &committer.date);
    }
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(message.as_bytes()).await?;
    drop(stdin);
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        bail!("commit-tree: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A parsed commit object.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub tree: String,
    pub parents: Vec<String>,
    pub author: RawIdent,
    pub committer: RawIdent,
    pub message: String,
}

impl CommitInfo {
    pub fn author_date(&self) -> DateTime<Utc> {
        parse_git_date(&self.author.date)
    }
    pub fn committer_date(&self) -> DateTime<Utc> {
        parse_git_date(&self.committer.date)
    }
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

pub fn parse_git_date(d: &str) -> DateTime<Utc> {
    d.split_whitespace()
        .next()
        .and_then(|t| t.parse::<i64>().ok())
        .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

fn parse_ident(s: &str) -> RawIdent {
    // "Name <email> 1700000000 +0200"
    let (name, rest) = s.split_once(" <").unwrap_or((s, ""));
    let (email, date) = rest.split_once("> ").unwrap_or((rest.trim_end_matches('>'), ""));
    RawIdent { name: name.to_string(), email: email.to_string(), date: date.trim().to_string() }
}

/// Parses `git cat-file commit` output.
pub fn parse_commit(sha: &str, raw: &[u8]) -> CommitInfo {
    let text = String::from_utf8_lossy(raw);
    let (header, message) = text.split_once("\n\n").unwrap_or((&text, ""));
    let mut c = CommitInfo {
        sha: sha.to_string(),
        tree: String::new(),
        parents: vec![],
        author: RawIdent { name: String::new(), email: String::new(), date: String::new() },
        committer: RawIdent { name: String::new(), email: String::new(), date: String::new() },
        message: message.to_string(),
    };
    for line in header.lines() {
        if let Some(v) = line.strip_prefix("tree ") {
            c.tree = v.to_string();
        } else if let Some(v) = line.strip_prefix("parent ") {
            c.parents.push(v.to_string());
        } else if let Some(v) = line.strip_prefix("author ") {
            c.author = parse_ident(v);
        } else if let Some(v) = line.strip_prefix("committer ") {
            c.committer = parse_ident(v);
        }
    }
    c
}

pub async fn read_commit(git: &GitCmd, repo: &Path, sha: &str) -> anyhow::Result<CommitInfo> {
    let raw = git.run(repo, &["cat-file", "commit", sha]).await?;
    Ok(parse_commit(sha, &raw))
}

/// Commits in `range` (e.g. `base..head`), oldest first, at most `limit`.
pub async fn list_commits(git: &GitCmd, repo: &Path, range: &str, limit: usize) -> anyhow::Result<Vec<CommitInfo>> {
    let shas = out_str(git, repo, &["rev-list", "--reverse", "--topo-order", range]).await?;
    let shas: Vec<String> = shas.lines().map(str::to_string).take(limit).collect();
    if shas.is_empty() {
        return Ok(vec![]);
    }
    // One cat-file --batch for all commits.
    let input = shas.join("\n") + "\n";
    let out = git.run_input(repo, &["cat-file", "--batch"], input.as_bytes()).await?;
    let mut res = Vec::with_capacity(shas.len());
    let mut i = 0;
    while i < out.len() {
        let nl = out[i..].iter().position(|b| *b == b'\n').context("cat-file header")? + i;
        let header = String::from_utf8_lossy(&out[i..nl]).to_string();
        let mut parts = header.split_whitespace();
        let sha = parts.next().unwrap_or("").to_string();
        let kind = parts.next().unwrap_or("");
        let size: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if kind == "missing" {
            i = nl + 1;
            continue;
        }
        let body = &out[nl + 1..nl + 1 + size];
        res.push(parse_commit(&sha, body));
        i = nl + 1 + size + 1;
    }
    Ok(res)
}

/// Replays the commits of `base..head` on top of `onto` (cherry-pick semantics, authors kept,
/// committer = `committer`). Returns the new tip, or None on conflict.
pub async fn rebase(git: &GitCmd, repo: &Path, onto: &str, base: &str, head: &str, committer: &RawIdent) -> anyhow::Result<Option<String>> {
    let commits = list_commits(git, repo, &format!("{base}..{head}"), usize::MAX).await?;
    let mut tip = onto.to_string();
    for c in commits {
        if c.parents.len() > 1 {
            return Ok(None);
        }
        let parent = c.parents.first().cloned().unwrap_or_else(|| EMPTY_TREE.to_string());
        let Some(tree) = merge_tree(git, repo, &tip, &c.sha, Some(&parent)).await? else { return Ok(None) };
        tip = commit_tree(git, repo, &tree, &[&tip], &c.message, &c.author, committer).await?;
    }
    Ok(Some(tip))
}

// ---------------------------------------------------------------------------------------------
// Diffs

#[derive(Debug, Clone, Default)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    /// added | removed | modified | renamed | copied | changed
    pub status: String,
    pub old_sha: Option<String>,
    pub new_sha: Option<String>,
    pub additions: i64,
    pub deletions: i64,
    pub binary: bool,
    /// Unified diff body starting at the first `@@` (GitHub's `patch`), empty for binary.
    pub patch: String,
}

impl FileDiff {
    pub fn filename(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("")
    }
}

fn unquote(p: &str) -> String {
    let p = p.trim_end_matches('\t');
    if !(p.starts_with('"') && p.ends_with('"') && p.len() >= 2) {
        return p.to_string();
    }
    let inner = &p[1..p.len() - 1];
    let mut bytes = Vec::new();
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            let c = b[i + 1];
            match c {
                b'n' => bytes.push(b'\n'),
                b't' => bytes.push(b'\t'),
                b'"' => bytes.push(b'"'),
                b'\\' => bytes.push(b'\\'),
                b'0'..=b'7' if i + 4 <= b.len() => {
                    let oct = std::str::from_utf8(&b[i + 1..i + 4]).unwrap_or("0");
                    bytes.push(u8::from_str_radix(oct, 8).unwrap_or(b'?'));
                    i += 4;
                    continue;
                }
                other => bytes.push(other),
            }
            i += 2;
        } else {
            bytes.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn strip_prefix_path(p: &str, prefix: &str) -> Option<String> {
    let p = unquote(p);
    if p == "/dev/null" {
        return None;
    }
    Some(p.strip_prefix(prefix).unwrap_or(&p).to_string())
}

/// Parses `git diff --full-index -M --src-prefix=a/ --dst-prefix=b/` output.
pub fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut in_hunks = false;
    let mut patch_lines: Vec<&str> = Vec::new();
    let finish = |cur: &mut Option<FileDiff>, patch_lines: &mut Vec<&str>, files: &mut Vec<FileDiff>| {
        if let Some(mut f) = cur.take() {
            f.patch = patch_lines.join("\n");
            if f.status.is_empty() {
                f.status = if f.patch.is_empty() && !f.binary { "changed".into() } else { "modified".into() };
            }
            files.push(f);
        }
        patch_lines.clear();
    };
    for line in text.split('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            finish(&mut cur, &mut patch_lines, &mut files);
            in_hunks = false;
            let mut f = FileDiff::default();
            // "a/P b/P" for non-renames (both halves equal).
            if !rest.starts_with('"') && rest.len() > 4 {
                let half = (rest.len() - 1) / 2;
                let (l, r) = (&rest[..half], &rest[half + 1..]);
                if l.len() > 2 && r.len() > 2 && l[2..] == r[2..] {
                    f.old_path = Some(l[2..].to_string());
                    f.new_path = Some(r[2..].to_string());
                }
            }
            cur = Some(f);
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if in_hunks {
            if line.starts_with(['+', '-', ' ', '\\', '@']) || line.is_empty() {
                if line.is_empty() {
                    // trailing newline of the whole output
                    continue;
                }
                if line.starts_with('+') {
                    f.additions += 1;
                } else if line.starts_with('-') {
                    f.deletions += 1;
                }
                patch_lines.push(line);
            }
            continue;
        }
        if line.starts_with("@@") {
            in_hunks = true;
            patch_lines.push(line);
        } else if line.starts_with("new file mode") {
            f.status = "added".into();
            f.old_path = None;
        } else if line.starts_with("deleted file mode") {
            f.status = "removed".into();
            f.new_path = None;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.status = "renamed".into();
            f.old_path = Some(unquote(p));
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.new_path = Some(unquote(p));
        } else if let Some(p) = line.strip_prefix("copy from ") {
            f.status = "copied".into();
            f.old_path = Some(unquote(p));
        } else if let Some(p) = line.strip_prefix("copy to ") {
            f.new_path = Some(unquote(p));
        } else if let Some(idx) = line.strip_prefix("index ") {
            let range = idx.split_whitespace().next().unwrap_or("");
            if let Some((a, b)) = range.split_once("..") {
                f.old_sha = Some(a.to_string()).filter(|s| s.trim_matches('0') != "");
                f.new_sha = Some(b.to_string()).filter(|s| s.trim_matches('0') != "");
            }
        } else if let Some(p) = line.strip_prefix("--- ") {
            if f.status != "added" {
                f.old_path = strip_prefix_path(p, "a/").or(f.old_path.take());
            }
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if f.status != "removed" {
                f.new_path = strip_prefix_path(p, "b/").or(f.new_path.take());
            }
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            f.binary = true;
        }
    }
    finish(&mut cur, &mut patch_lines, &mut files);
    files
}

pub async fn diff(git: &GitCmd, repo: &Path, from: &str, to: &str, path: Option<&str>) -> anyhow::Result<Vec<FileDiff>> {
    let text = raw_diff(git, repo, from, to, path).await?;
    Ok(parse_diff(&String::from_utf8_lossy(&text)))
}

/// The unified diff GitHub serves for `application/vnd.github.diff`.
pub async fn raw_diff(git: &GitCmd, repo: &Path, from: &str, to: &str, path: Option<&str>) -> anyhow::Result<Vec<u8>> {
    let mut args = vec![
        "-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff", "--full-index", "-M",
        "--src-prefix=a/", "--dst-prefix=b/", from, to,
    ];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    git.run(repo, &args).await
}

/// `git format-patch --stdout` for `application/vnd.github.patch`.
pub async fn format_patch(git: &GitCmd, repo: &Path, range: &str) -> anyhow::Result<Vec<u8>> {
    git.run(repo, &["format-patch", "--stdout", "--no-color", range]).await
}

/// One line of a file patch with its diff position and line numbers.
#[derive(Debug, Clone)]
pub struct PatchLine {
    pub position: i64,
    pub kind: char,
    pub old_line: Option<i64>,
    pub new_line: Option<i64>,
    /// index of the enclosing hunk header in the patch lines
    pub hunk_start: usize,
    pub index: usize,
}

fn parse_hunk_header(h: &str) -> (i64, i64) {
    // @@ -a,b +c,d @@
    let mut old = 0;
    let mut new = 0;
    for part in h.split_whitespace() {
        if let Some(v) = part.strip_prefix('-') {
            old = v.split(',').next().and_then(|x| x.parse().ok()).unwrap_or(0);
        } else if let Some(v) = part.strip_prefix('+') {
            new = v.split(',').next().and_then(|x| x.parse().ok()).unwrap_or(0);
        }
    }
    (old, new)
}

/// Walks a GitHub `patch` assigning positions (line below the first @@ = 1; later hunk headers
/// count as positions too) and old/new line numbers.
pub fn patch_lines(patch: &str) -> Vec<PatchLine> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0i64, 0i64);
    let mut hunk_start = 0;
    for (i, line) in patch.split('\n').enumerate() {
        let kind = line.chars().next().unwrap_or(' ');
        match kind {
            '@' => {
                let (o, n) = parse_hunk_header(line);
                old = o;
                new = n;
                hunk_start = i;
                if i > 0 {
                    out.push(PatchLine { position: i as i64, kind, old_line: None, new_line: None, hunk_start, index: i });
                }
            }
            '+' => {
                out.push(PatchLine { position: i as i64, kind, old_line: None, new_line: Some(new), hunk_start, index: i });
                new += 1;
            }
            '-' => {
                out.push(PatchLine { position: i as i64, kind, old_line: Some(old), new_line: None, hunk_start, index: i });
                old += 1;
            }
            '\\' => {}
            _ => {
                out.push(PatchLine { position: i as i64, kind: ' ', old_line: Some(old), new_line: Some(new), hunk_start, index: i });
                old += 1;
                new += 1;
            }
        }
    }
    out
}

/// The diff hunk GitHub stores with a review comment: hunk header through the commented line.
pub fn diff_hunk(patch: &str, pl: &PatchLine) -> String {
    patch.split('\n').skip(pl.hunk_start).take(pl.index - pl.hunk_start + 1).collect::<Vec<_>>().join("\n")
}

/// Finds the patch line for `line` on `side` ("LEFT" = old file, "RIGHT" = new file).
pub fn find_line(patch: &str, line: i64, side: &str) -> Option<PatchLine> {
    patch_lines(patch).into_iter().find(|p| {
        if side == "LEFT" {
            p.kind != '+' && p.kind != '@' && p.old_line == Some(line)
        } else {
            p.kind != '-' && p.kind != '@' && p.new_line == Some(line)
        }
    })
}

pub fn find_position(patch: &str, position: i64) -> Option<PatchLine> {
    patch_lines(patch).into_iter().find(|p| p.position == position && p.kind != '@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff_and_positions() {
        let d = "diff --git a/f.txt b/f.txt\nindex 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -10,2 +10,3 @@\n x\n+y\n z\ndiff --git a/new file.txt b/new file.txt\nnew file mode 100644\nindex 0000000000000000000000000000000000000000..3333333333333333333333333333333333333333\n--- /dev/null\n+++ b/new file.txt\n@@ -0,0 +1 @@\n+hello\ndiff --git a/old.txt b/renamed.txt\nsimilarity index 100%\nrename from old.txt\nrename to renamed.txt\n";
        let f = parse_diff(d);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].status, "modified");
        assert_eq!((f[0].additions, f[0].deletions), (2, 1));
        assert!(f[0].patch.starts_with("@@ -1,3"));
        assert_eq!(f[1].status, "added");
        assert_eq!(f[1].filename(), "new file.txt");
        assert_eq!(f[2].status, "renamed");
        assert_eq!(f[2].old_path.as_deref(), Some("old.txt"));
        assert_eq!(f[2].filename(), "renamed.txt");
        // positions: " a"=1 "-b"=2 "+B"=3 " c"=4 "@@"=5 " x"=6 "+y"=7
        let pl = find_line(&f[0].patch, 11, "RIGHT").unwrap();
        assert_eq!(pl.position, 7);
        assert_eq!(diff_hunk(&f[0].patch, &pl), "@@ -10,2 +10,3 @@\n x\n+y");
        let pl = find_line(&f[0].patch, 2, "LEFT").unwrap();
        assert_eq!(pl.position, 2);
        assert_eq!(find_position(&f[0].patch, 3).unwrap().new_line, Some(2));
    }

    #[test]
    fn parses_commit() {
        let c = parse_commit("abc", b"tree t\nparent p1\nauthor A B <a@b> 1700000000 +0200\ncommitter C <c@d> 1700000001 +0000\n\nsubject\n\nbody\n");
        assert_eq!(c.author.name, "A B");
        assert_eq!(c.author.email, "a@b");
        assert_eq!(c.author.date, "1700000000 +0200");
        assert_eq!(c.parents, vec!["p1"]);
        assert_eq!(c.subject(), "subject");
    }
}

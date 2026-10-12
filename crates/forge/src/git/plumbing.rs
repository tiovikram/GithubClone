//! Object-level helpers that work on a bare repository (no working tree).
use std::path::Path;

use super::cmd::{GitCmd, Identity};

/// A file change for `commit_changes`.
pub enum Change {
    /// Write `content` at `path` with the given mode (e.g. "100644", "100755").
    Put { path: String, content: Vec<u8>, mode: String },
    Delete { path: String },
}

/// Creates a commit on top of `parent` (or a root commit) applying `changes`, using a temporary
/// index so concurrent readers are unaffected. Does not move any ref. Returns the commit sha.
pub async fn commit_changes(
    git: &GitCmd,
    repo: &Path,
    parent: Option<&str>,
    changes: Vec<Change>,
    message: &str,
    author: &Identity,
    committer: &Identity,
) -> anyhow::Result<String> {
    let index = repo.join(format!("forge-index-{}", crate::auth::random_string(10)));
    let res = async {
        let idx = index.to_str().unwrap().to_string();
        let run_idx = |args: Vec<String>, input: Option<Vec<u8>>| {
            let mut c = git.command(repo);
            c.env("GIT_INDEX_FILE", &idx).args(&args);
            async move {
                use std::process::Stdio;
                use tokio::io::AsyncWriteExt;
                c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                let mut child = c.spawn()?;
                if let Some(data) = input {
                    let mut stdin = child.stdin.take().unwrap();
                    stdin.write_all(&data).await?;
                    drop(stdin);
                }
                let out = child.wait_with_output().await?;
                if !out.status.success() {
                    anyhow::bail!("git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
                }
                Ok::<_, anyhow::Error>(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
        };
        match parent {
            Some(p) => run_idx(vec!["read-tree".into(), p.into()], None).await?,
            None => run_idx(vec!["read-tree".into(), "--empty".into()], None).await?,
        };
        for ch in changes {
            match ch {
                Change::Put { path, content, mode } => {
                    let sha = git
                        .run_input(repo, &["hash-object", "-w", "--stdin"], &content)
                        .await
                        .map(|b| String::from_utf8_lossy(&b).trim().to_string())?;
                    run_idx(
                        vec!["update-index".into(), "--add".into(), "--cacheinfo".into(), format!("{mode},{sha},{path}")],
                        None,
                    )
                    .await?;
                }
                Change::Delete { path } => {
                    run_idx(vec!["update-index".into(), "--force-remove".into(), "--".into(), path], None).await?;
                }
            }
        }
        let tree = run_idx(vec!["write-tree".into()], None).await?;
        commit_tree(git, repo, &tree, parent.into_iter().collect(), message, author, committer).await
    }
    .await;
    let _ = tokio::fs::remove_file(&index).await;
    res
}

/// `git commit-tree` with explicit author and committer.
pub async fn commit_tree(
    git: &GitCmd,
    repo: &Path,
    tree: &str,
    parents: Vec<&str>,
    message: &str,
    author: &Identity,
    committer: &Identity,
) -> anyhow::Result<String> {
    let mut args: Vec<String> = vec!["commit-tree".into(), tree.into()];
    for p in parents {
        args.push("-p".into());
        args.push(p.into());
    }
    let mut c = git.command(repo);
    c.args(&args)
        .env("GIT_AUTHOR_NAME", &author.name)
        .env("GIT_AUTHOR_EMAIL", &author.email)
        .env("GIT_COMMITTER_NAME", &committer.name)
        .env("GIT_COMMITTER_EMAIL", &committer.email);
    if let Some(t) = author.when {
        c.env("GIT_AUTHOR_DATE", format!("{t} +0000"));
    }
    if let Some(t) = committer.when {
        c.env("GIT_COMMITTER_DATE", format!("{t} +0000"));
    }
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(message.as_bytes()).await?;
    drop(stdin);
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        anyhow::bail!("commit-tree: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Atomically moves `refname` to `new` if it currently equals `old` (None = must not exist).
pub async fn update_ref(git: &GitCmd, repo: &Path, refname: &str, new: &str, old: Option<&str>) -> anyhow::Result<()> {
    const ZERO: &str = "0000000000000000000000000000000000000000";
    git.run(repo, &["update-ref", refname, new, old.unwrap_or(ZERO)]).await?;
    Ok(())
}

pub async fn delete_ref(git: &GitCmd, repo: &Path, refname: &str) -> anyhow::Result<()> {
    git.run(repo, &["update-ref", "-d", refname]).await?;
    Ok(())
}

/// Resolves a revision to a full sha, or None.
pub async fn rev_parse(git: &GitCmd, repo: &Path, rev: &str) -> Option<String> {
    let (ok, out, _) = git.try_run(repo, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).await.ok()?;
    ok.then(|| String::from_utf8_lossy(&out).trim().to_string()).filter(|s| !s.is_empty())
}

//! Thin async wrapper around the `git` executable.
use std::path::Path;
use std::process::Stdio;

use anyhow::bail;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Clone, Debug)]
pub struct GitCmd {
    bin: String,
}

/// Identity used for commits forge creates (merges, API file edits).
#[derive(Clone, Debug)]
pub struct Identity {
    pub name: String,
    pub email: String,
    /// Unix seconds; `None` = now.
    pub when: Option<i64>,
}

impl GitCmd {
    pub fn new(bin: String) -> Self {
        GitCmd { bin }
    }

    /// A `git` command isolated from user/system configuration.
    pub fn command(&self, dir: &Path) -> Command {
        let mut c = Command::new(&self.bin);
        c.current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .kill_on_drop(true);
        c
    }

    /// Runs git and returns stdout; non-zero exit is an error carrying stderr.
    pub async fn run(&self, dir: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
        self.run_full(dir, args, None, None).await
    }

    pub async fn run_input(&self, dir: &Path, args: &[&str], input: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.run_full(dir, args, Some(input), None).await
    }

    pub async fn run_as(&self, dir: &Path, args: &[&str], input: Option<&[u8]>, who: &Identity) -> anyhow::Result<Vec<u8>> {
        self.run_full(dir, args, input, Some(who)).await
    }

    /// Like `run` but returns stdout as a trimmed string.
    pub async fn run_str(&self, dir: &Path, args: &[&str]) -> anyhow::Result<String> {
        Ok(String::from_utf8_lossy(&self.run(dir, args).await?).trim_end().to_string())
    }

    /// Runs git, returning (exit success, stdout, stderr) without treating failure as an error.
    pub async fn try_run(&self, dir: &Path, args: &[&str]) -> anyhow::Result<(bool, Vec<u8>, Vec<u8>)> {
        let out = self.command(dir).args(args).stdin(Stdio::null()).output().await?;
        Ok((out.status.success(), out.stdout, out.stderr))
    }

    async fn run_full(&self, dir: &Path, args: &[&str], input: Option<&[u8]>, who: Option<&Identity>) -> anyhow::Result<Vec<u8>> {
        let mut c = self.command(dir);
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(w) = who {
            let date = w.when.map(|t| format!("{t} +0000")).unwrap_or_default();
            c.env("GIT_AUTHOR_NAME", &w.name)
                .env("GIT_AUTHOR_EMAIL", &w.email)
                .env("GIT_COMMITTER_NAME", &w.name)
                .env("GIT_COMMITTER_EMAIL", &w.email);
            if !date.is_empty() {
                c.env("GIT_AUTHOR_DATE", &date).env("GIT_COMMITTER_DATE", &date);
            }
        }
        c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
        let mut child = c.spawn()?;
        if let Some(data) = input {
            let mut stdin = child.stdin.take().unwrap();
            let data = data.to_vec();
            tokio::spawn(async move {
                let _ = stdin.write_all(&data).await;
            });
        }
        let out = child.wait_with_output().await?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(out.stdout)
    }
}

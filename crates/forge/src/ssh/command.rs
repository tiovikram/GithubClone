//! Parsing of SSH exec requests and the small amount of pkt-line handling the SSH push path
//! needs (splitting receive-pack's advertisement from its report, synthesizing a failure report).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    UploadPack,
    ReceivePack,
    UploadArchive,
}

impl Service {
    pub fn git_subcommand(self) -> &'static str {
        match self {
            Service::UploadPack => "upload-pack",
            Service::ReceivePack => "receive-pack",
            Service::UploadArchive => "upload-archive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommand {
    pub service: Service,
    pub owner: String,
    /// Repository name without a `.git` suffix.
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Not a git command at all (e.g. `ls`).
    NotGit,
    /// A git command with a path that is not `owner/repo(.git)`.
    BadPath,
}

/// Parses `git-upload-pack '/owner/repo.git'` and its variants: `git upload-pack`, single or
/// double quotes or none, optional leading `/` (or `~/`), optional `.git` suffix.
pub fn parse(cmd: &str) -> Result<GitCommand, ParseError> {
    let cmd = cmd.trim();
    let (verb, rest) = split_word(cmd);
    let (verb, rest) = if verb == "git" {
        let (sub, rest) = split_word(rest);
        (format!("git-{sub}"), rest)
    } else {
        (verb.to_string(), rest)
    };
    let service = match verb.as_str() {
        "git-upload-pack" => Service::UploadPack,
        "git-receive-pack" => Service::ReceivePack,
        "git-upload-archive" => Service::UploadArchive,
        _ => return Err(ParseError::NotGit),
    };
    let arg = unquote(rest.trim()).ok_or(ParseError::BadPath)?;
    let path = arg.strip_prefix("~/").unwrap_or(&arg);
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(ParseError::BadPath);
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !ok(owner) || !ok(repo) {
        return Err(ParseError::BadPath);
    }
    Ok(GitCommand { service, owner: owner.to_string(), repo: repo.to_string() })
}

fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

/// Removes one level of shell quoting: `'x'`, `"x"` or a bare word. Embedded quotes,
/// whitespace and escapes are rejected (repository paths never need them).
fn unquote(s: &str) -> Option<String> {
    let inner = if (s.starts_with('\'') && s.ends_with('\'') || s.starts_with('"') && s.ends_with('"')) && s.len() >= 2 {
        &s[1..s.len() - 1]
    } else {
        s
    };
    if inner.is_empty() || inner.chars().any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\' | '`' | '$')) {
        return None;
    }
    Some(inner.to_string())
}

/// Validates a `GIT_PROTOCOL` value (e.g. `version=2`, `version=2:object-format=sha1`).
pub fn valid_git_protocol(v: &str) -> bool {
    !v.is_empty() && v.len() <= 256 && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '=' | ':' | '-' | '_' | '.'))
}

pub fn pkt_line(payload: &[u8]) -> Vec<u8> {
    let mut v = format!("{:04x}", payload.len() + 4).into_bytes();
    v.extend_from_slice(payload);
    v
}

/// Incremental pkt-line scanner: reports the byte offset just past the first flush-pkt.
#[derive(Default)]
pub struct FlushFinder {
    /// Bytes of the current pkt header seen so far.
    header: Vec<u8>,
    /// Payload bytes still to skip in the current pkt.
    remaining: usize,
    pub done: bool,
    pub broken: bool,
}

impl FlushFinder {
    /// Feeds `buf`; returns `Some(n)` if the first flush-pkt ends at `buf[..n]`.
    pub fn feed(&mut self, buf: &[u8]) -> Option<usize> {
        let mut i = 0;
        while i < buf.len() && !self.done && !self.broken {
            if self.remaining > 0 {
                let take = self.remaining.min(buf.len() - i);
                self.remaining -= take;
                i += take;
                continue;
            }
            self.header.push(buf[i]);
            i += 1;
            if self.header.len() == 4 {
                let len = std::str::from_utf8(&self.header).ok().and_then(|h| usize::from_str_radix(h, 16).ok());
                self.header.clear();
                match len {
                    Some(0) => {
                        self.done = true;
                        return Some(i);
                    }
                    Some(1..=3) => {} // delim / response-end: no payload
                    Some(n) => self.remaining = n - 4,
                    None => self.broken = true,
                }
            }
        }
        None
    }
}

/// What the client asked for in its receive-pack command list.
#[derive(Debug, Default, Clone)]
pub struct PushRequest {
    pub refs: Vec<String>,
    pub report_status: bool,
    pub side_band: bool,
    /// The command list ended (flush-pkt seen).
    pub complete: bool,
}

/// Incremental parser for the client side of a receive-pack exchange: collects the ref names
/// and capabilities from the command list (pkt-lines up to the first flush).
#[derive(Default)]
pub struct PushRequestParser {
    buf: Vec<u8>,
    pub req: PushRequest,
    broken: bool,
}

impl PushRequestParser {
    pub fn feed(&mut self, data: &[u8]) {
        if self.req.complete || self.broken {
            return;
        }
        self.buf.extend_from_slice(data);
        loop {
            if self.buf.len() < 4 {
                return;
            }
            let Some(len) = std::str::from_utf8(&self.buf[..4]).ok().and_then(|h| usize::from_str_radix(h, 16).ok())
            else {
                self.broken = true;
                return;
            };
            if len == 0 {
                self.req.complete = true;
                self.buf.clear();
                return;
            }
            if len < 4 {
                self.broken = true;
                return;
            }
            if self.buf.len() < len {
                return;
            }
            let line: Vec<u8> = self.buf.drain(..len).skip(4).collect();
            let (cmd, caps) = match line.iter().position(|&b| b == 0) {
                Some(p) => (&line[..p], Some(&line[p + 1..])),
                None => (&line[..], None),
            };
            if let Some(caps) = caps {
                let caps = String::from_utf8_lossy(caps);
                for c in caps.split_whitespace() {
                    match c {
                        "report-status" | "report-status-v2" => self.req.report_status = true,
                        "side-band-64k" | "side-band" => self.req.side_band = true,
                        _ => {}
                    }
                }
            }
            let cmd = String::from_utf8_lossy(cmd);
            // "<old> <new> <ref>" (shallow/push-cert lines are ignored)
            let parts: Vec<&str> = cmd.trim_end().splitn(3, ' ').collect();
            if parts.len() == 3 && parts[0].len() >= 40 && parts[1].len() >= 40 {
                self.req.refs.push(parts[2].to_string());
            }
        }
    }
}

/// A report-status response rejecting every ref with `reason` (used when the storage commit
/// fails after receive-pack already accepted the push), wrapped in side-band #1 if negotiated.
pub fn failure_report(req: &PushRequest, reason: &str) -> Vec<u8> {
    let mut inner = pkt_line(b"unpack ok\n");
    for r in &req.refs {
        inner.extend(pkt_line(format!("ng {r} {reason}\n").as_bytes()));
    }
    inner.extend_from_slice(b"0000");
    if !req.side_band {
        return inner;
    }
    let mut out = Vec::new();
    for chunk in inner.chunks(65515) {
        let mut p = vec![1u8];
        p.extend_from_slice(chunk);
        out.extend(pkt_line(&p));
    }
    out.extend_from_slice(b"0000");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_variants() {
        let want = |s: Service| GitCommand { service: s, owner: "o".into(), repo: "r".into() };
        assert_eq!(parse("git-upload-pack '/o/r.git'"), Ok(want(Service::UploadPack)));
        assert_eq!(parse("git-receive-pack 'o/r.git'"), Ok(want(Service::ReceivePack)));
        assert_eq!(parse("git-receive-pack 'o/r'"), Ok(want(Service::ReceivePack)));
        assert_eq!(parse("git upload-pack \"o/r.git\""), Ok(want(Service::UploadPack)));
        assert_eq!(parse("git-upload-archive /o/r"), Ok(want(Service::UploadArchive)));
        assert_eq!(parse("git-upload-pack '~/o/r.git'"), Ok(want(Service::UploadPack)));
        assert_eq!(parse("ls -la"), Err(ParseError::NotGit));
        assert_eq!(parse(""), Err(ParseError::NotGit));
        assert_eq!(parse("git-upload-pack '../etc/passwd'"), Err(ParseError::BadPath));
        assert_eq!(parse("git-upload-pack 'a/b/c'"), Err(ParseError::BadPath));
        assert_eq!(parse("git-upload-pack 'o/r'; rm -rf /"), Err(ParseError::BadPath));
        assert_eq!(parse("git-upload-pack"), Err(ParseError::BadPath));
    }

    #[test]
    fn finds_flush_across_chunks() {
        let mut data = pkt_line(b"abc 123\n");
        data.extend(pkt_line(b"def\n"));
        data.extend(b"0000");
        let tail = b"PACK...";
        let mut all = data.clone();
        all.extend(tail);
        for split in 0..all.len() {
            let mut f = FlushFinder::default();
            let a = f.feed(&all[..split]);
            let pos = match a {
                Some(n) => n,
                None => split + f.feed(&all[split..]).unwrap(),
            };
            assert_eq!(pos, data.len(), "split {split}");
        }
    }

    #[test]
    fn parses_push_commands_and_builds_failure_report() {
        let z = "0".repeat(40);
        let n = "1".repeat(40);
        let mut data = pkt_line(format!("{z} {n} refs/heads/main\0report-status side-band-64k agent=git/2\n").as_bytes());
        data.extend(pkt_line(format!("{n} {z} refs/heads/old\n").as_bytes()));
        data.extend(b"0000PACK");
        let mut p = PushRequestParser::default();
        for c in data.chunks(7) {
            p.feed(c);
        }
        assert!(p.req.complete && p.req.report_status && p.req.side_band);
        assert_eq!(p.req.refs, vec!["refs/heads/main", "refs/heads/old"]);
        let rep = failure_report(&p.req, "storage commit failed");
        assert!(rep.starts_with(b"00"));
        assert!(rep.ends_with(b"0000"));
        let s = String::from_utf8_lossy(&rep);
        assert!(s.contains("ng refs/heads/main storage commit failed"));
    }
}

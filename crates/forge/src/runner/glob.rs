//! Filter patterns of workflow triggers (`branches`, `tags`, `paths`) and of `hashFiles`.
//!
//! `*` matches any run of characters except `/`, `**` matches anything, `?` and `+` quantify the
//! preceding character (zero-or-one / one-or-more, as GitHub documents), `[...]` is a character
//! class and `\` escapes. A leading `!` negates (handled by [`matches_filters`]).
use regex::Regex;

pub fn to_regex(pattern: &str) -> Option<Regex> {
    let c: Vec<char> = pattern.chars().collect();
    let mut re = String::from("^");
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '*' => {
                if i + 1 < c.len() && c[i + 1] == '*' {
                    // `**/` also matches zero directories
                    if i + 2 < c.len() && c[i + 2] == '/' {
                        re.push_str("(?:.*/)?");
                        i += 3;
                        continue;
                    }
                    re.push_str(".*");
                    i += 2;
                } else {
                    re.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                re.push('?');
                i += 1;
            }
            '+' => {
                re.push('+');
                i += 1;
            }
            '[' => {
                let end = c[i..].iter().position(|&x| x == ']').map(|p| p + i)?;
                let class: String = c[i + 1..end].iter().collect();
                re.push('[');
                re.push_str(&class.replace('\\', "\\\\"));
                re.push(']');
                i = end + 1;
            }
            '\\' if i + 1 < c.len() => {
                re.push_str(&regex::escape(&c[i + 1].to_string()));
                i += 2;
            }
            ch => {
                re.push_str(&regex::escape(&ch.to_string()));
                i += 1;
            }
        }
    }
    re.push('$');
    Regex::new(&re).ok()
}

pub fn glob_match(pattern: &str, s: &str) -> bool {
    to_regex(pattern).is_some_and(|r| r.is_match(s))
}

/// Applies an ordered list of patterns where `!pattern` excludes again (GitHub semantics: the
/// last matching pattern wins). With only negative patterns, everything else matches.
pub fn matches_filters(patterns: &[String], s: &str) -> bool {
    let mut matched = patterns.iter().all(|p| p.starts_with('!'));
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob_match(neg, s) {
                matched = false;
            }
        } else if glob_match(p, s) {
            matched = true;
        }
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(glob_match("main", "main"));
        assert!(glob_match("releases/*", "releases/v1"));
        assert!(!glob_match("releases/*", "releases/v1/x"));
        assert!(glob_match("releases/**", "releases/v1/x"));
        assert!(glob_match("**.js", "a/b/c.js"));
        assert!(glob_match("**/*.md", "README.md"));
        assert!(glob_match("v[12].*", "v1.0"));
        assert!(glob_match("v1.?0", "v1.0"));
        assert!(matches_filters(&["main".into(), "dev/**".into()], "dev/x/y"));
        assert!(!matches_filters(&["**".into(), "!docs/**".into()], "docs/a.md"));
        assert!(matches_filters(&["!docs/**".into()], "src/a.rs"));
    }
}

//! Secret scanning patterns.
//!
//! Provider patterns (GitHub's `secret_type` slugs) are always scanned when secret scanning is
//! enabled. Private keys in PEM/OpenSSH/PGP armor are scanned too. The *non-provider* patterns
//! (connection strings, HTTP authorization headers) are only scanned when
//! `secret_scanning_non_provider_patterns` is enabled, as on GitHub.
//!
//! Patterns are plain `regex` expressions (no look-around); `boundary` patterns additionally
//! require that the match is not glued to surrounding identifier characters, so e.g. a 36-char
//! `ghp_` token inside a longer random string is not reported.
use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, Copy)]
pub struct Pattern {
    pub slug: &'static str,
    pub display_name: &'static str,
    pub provider: Option<&'static str>,
    pub provider_slug: Option<&'static str>,
    pub regex: &'static str,
    /// Capture group holding the secret (0 = the whole match).
    pub group: usize,
    pub boundary: bool,
    pub non_provider: bool,
    /// Push protection blocks this type by default.
    pub push_protected: bool,
}

const fn p(slug: &'static str, display_name: &'static str, provider: &'static str, provider_slug: &'static str, regex: &'static str) -> Pattern {
    Pattern { slug, display_name, provider: Some(provider), provider_slug: Some(provider_slug), regex, group: 0, boundary: true, non_provider: false, push_protected: true }
}

const fn g(mut x: Pattern, group: usize) -> Pattern {
    x.group = group;
    x.boundary = false;
    x
}

const fn generic(slug: &'static str, display_name: &'static str, regex: &'static str, group: usize, non_provider: bool) -> Pattern {
    Pattern { slug, display_name, provider: None, provider_slug: None, regex, group, boundary: false, non_provider, push_protected: !non_provider }
}

pub const PATTERNS: &[Pattern] = &[
    // GitHub (and forge's own token formats, which use the same prefixes).
    p("github_personal_access_token", "GitHub Personal Access Token", "GitHub", "github", r"ghp_[A-Za-z0-9]{36}"),
    p("github_personal_access_token", "GitHub Personal Access Token", "GitHub", "github", r"github_pat_[A-Za-z0-9]{22}_[A-Za-z0-9]{59}"),
    p("github_oauth_access_token", "GitHub OAuth Access Token", "GitHub", "github", r"gho_[A-Za-z0-9]{36}"),
    p("github_app_installation_access_token", "GitHub App Installation Access Token", "GitHub", "github", r"ghs_[A-Za-z0-9]{36}"),
    p("github_app_user_to_server_token", "GitHub App User To Server Token", "GitHub", "github", r"ghu_[A-Za-z0-9]{36}"),
    p("github_refresh_token", "GitHub Refresh Token", "GitHub", "github", r"ghr_[A-Za-z0-9]{36,76}"),
    // Amazon AWS
    p("aws_access_key_id", "Amazon AWS Access Key ID", "Amazon AWS", "amazon_aws", r"(?:AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}"),
    g(p("aws_secret_access_key", "Amazon AWS Secret Access Key", "Amazon AWS", "amazon_aws",
        r#"(?i)aws_?secret_?(?:access_?)?key["']?\s*[:=]\s*["']?([A-Za-z0-9/+]{40})(?:[^A-Za-z0-9/+]|$)"#), 1),
    // Slack
    p("slack_api_token", "Slack API Token", "Slack", "slack", r"xox[baprs]-[0-9A-Za-z-]{10,72}"),
    p("slack_incoming_webhook_url", "Slack Incoming Webhook URL", "Slack", "slack", r"https://hooks\.slack\.com/services/T[A-Z0-9]{8,12}/B[A-Z0-9]{8,12}/[A-Za-z0-9]{24}"),
    // Stripe
    p("stripe_api_key", "Stripe API Key", "Stripe", "stripe", r"sk_live_[0-9a-zA-Z]{24,99}"),
    p("stripe_test_secret_key", "Stripe Test API Secret Key", "Stripe", "stripe", r"sk_test_[0-9a-zA-Z]{24,99}"),
    p("stripe_live_restricted_key", "Stripe Live API Restricted Key", "Stripe", "stripe", r"rk_live_[0-9a-zA-Z]{24,99}"),
    // Google
    p("google_api_key", "Google API Key", "Google", "google", r"AIza[0-9A-Za-z_\-]{35}"),
    p("google_oauth_client_secret", "Google OAuth Client Secret", "Google", "google", r"GOCSPX-[A-Za-z0-9_\-]{28}"),
    // Package registries
    p("npm_access_token", "npm Access Token", "npm", "npm", r"npm_[A-Za-z0-9]{36}"),
    p("pypi_api_token", "PyPI API Token", "Python Package Index", "pypi", r"pypi-AgEIcHlwaS5vcmc[A-Za-z0-9_\-]{50,}"),
    p("docker_personal_access_token", "Docker Personal Access Token", "Docker", "docker", r"dckr_pat_[A-Za-z0-9_\-]{27}"),
    // Others
    p("gitlab_access_token", "GitLab Access Token", "GitLab", "gitlab", r"glpat-[A-Za-z0-9_\-]{20}"),
    p("sendgrid_api_key", "SendGrid API Key", "SendGrid", "sendgrid", r"SG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}"),
    p("twilio_api_key", "Twilio API Key", "Twilio", "twilio", r"SK[0-9a-fA-F]{32}"),
    p("shopify_access_token", "Shopify Access Token", "Shopify", "shopify", r"shpat_[a-fA-F0-9]{32}"),
    p("openai_api_key", "OpenAI API Key", "OpenAI", "openai", r"sk-(?:proj-)?[A-Za-z0-9_\-]{20,}T3BlbkFJ[A-Za-z0-9_\-]{20,}"),
    p("anthropic_api_key", "Anthropic API Key", "Anthropic", "anthropic", r"sk-ant-api03-[A-Za-z0-9_\-]{93}AA"),
    p("digitalocean_personal_access_token", "DigitalOcean Personal Access Token", "DigitalOcean", "digitalocean", r"dop_v1_[a-f0-9]{64}"),
    p("hashicorp_vault_service_token", "HashiCorp Vault Service Token", "HashiCorp", "hashicorp", r"hvs\.[A-Za-z0-9_\-]{24,}"),
    p("mailchimp_api_key", "Mailchimp API Key", "Mailchimp", "mailchimp", r"[0-9a-f]{32}-us[0-9]{1,2}"),
    p("telegram_bot_token", "Telegram Bot Token", "Telegram", "telegram", r"[0-9]{8,10}:AA[0-9A-Za-z_\-]{33}"),
    g(p("azure_storage_account_key", "Azure Storage Account Access Key", "Azure", "azure", r"AccountKey=([A-Za-z0-9+/]{86}==)"), 1),
    // Private keys (any provider).
    generic("rsa_private_key", "RSA Private Key", r"(?s)-----BEGIN RSA PRIVATE KEY-----.{16,}?-----END RSA PRIVATE KEY-----", 0, false),
    generic("openssh_private_key", "OpenSSH Private Key", r"(?s)-----BEGIN OPENSSH PRIVATE KEY-----.{16,}?-----END OPENSSH PRIVATE KEY-----", 0, false),
    generic("pgp_private_key", "PGP Private Key", r"(?s)-----BEGIN PGP PRIVATE KEY BLOCK-----.{16,}?-----END PGP PRIVATE KEY BLOCK-----", 0, false),
    generic("ec_private_key", "EC Private Key", r"(?s)-----BEGIN EC PRIVATE KEY-----.{16,}?-----END EC PRIVATE KEY-----", 0, false),
    generic("private_key", "Private Key", r"(?s)-----BEGIN (?:ENCRYPTED |DSA )?PRIVATE KEY-----.{16,}?-----END (?:ENCRYPTED |DSA )?PRIVATE KEY-----", 0, false),
    // Non-provider patterns.
    generic("http_basic_authentication_header", "HTTP Basic Authentication Header", r"(?i)authorization:\s*basic\s+([A-Za-z0-9+/]{8,}={0,2})", 1, true),
    generic("http_bearer_authentication_header", "HTTP Bearer Authentication Header", r"(?i)authorization:\s*bearer\s+([A-Za-z0-9\-._~+/]{20,}=*)", 1, true),
    generic("postgres_connection_string", "Postgres Connection String", r"postgres(?:ql)?://[^\s:@/]+:[^\s@/]+@[^\s/?#]+", 0, true),
    generic("mongodb_connection_string", "MongoDB Connection String", r"mongodb(?:\+srv)?://[^\s:@/]+:[^\s@/]+@[^\s/?#]+", 0, true),
    generic("mysql_connection_string", "MySQL Connection String", r"mysql://[^\s:@/]+:[^\s@/]+@[^\s/?#]+", 0, true),
];

static COMPILED: LazyLock<Vec<Regex>> = LazyLock::new(|| PATTERNS.iter().map(|p| Regex::new(p.regex).expect("secret pattern")).collect());

/// The pattern for a secret type slug (first match).
pub fn by_slug(slug: &str) -> Option<&'static Pattern> {
    PATTERNS.iter().find(|p| p.slug == slug)
}

/// Distinct secret types, in pattern order.
pub fn token_types() -> Vec<&'static Pattern> {
    let mut out: Vec<&'static Pattern> = Vec::new();
    for p in PATTERNS {
        if !out.iter().any(|q| q.slug == p.slug) {
            out.push(p);
        }
    }
    out
}

/// A secret found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub secret_type: &'static str,
    pub secret: String,
    /// 1-based line numbers; 1-based start column, end column exclusive (GitHub's convention).
    pub start_line: i32,
    pub end_line: i32,
    pub start_column: i32,
    pub end_column: i32,
}

fn ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Scans `text` for secrets. `non_provider` includes the generic non-provider patterns.
pub fn scan(text: &str, non_provider: bool) -> Vec<Finding> {
    let mut line_starts = vec![0usize];
    line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let pos = |off: usize| -> (i32, i32) {
        let line = line_starts.partition_point(|&s| s <= off) - 1;
        let col = text[line_starts[line]..off].chars().count() + 1;
        (line as i32 + 1, col as i32)
    };
    let mut out: Vec<Finding> = Vec::new();
    let mut covered: Vec<(usize, usize)> = Vec::new();
    for (p, re) in PATTERNS.iter().zip(COMPILED.iter()) {
        if p.non_provider && !non_provider {
            continue;
        }
        for caps in re.captures_iter(text) {
            let Some(m) = caps.get(p.group) else { continue };
            if p.boundary {
                let before = text[..m.start()].chars().next_back();
                let after = text[m.end()..].chars().next();
                if before.is_some_and(ident) || after.is_some_and(|c| c.is_ascii_alphanumeric()) {
                    continue;
                }
            }
            // A span already reported by an earlier (more specific) pattern is not reported again.
            if covered.iter().any(|&(s, e)| m.start() < e && s < m.end()) {
                continue;
            }
            covered.push((m.start(), m.end()));
            let (sl, sc) = pos(m.start());
            let (el, ec) = pos(m.end());
            out.push(Finding { secret_type: p.slug, secret: m.as_str().to_string(), start_line: sl, end_line: el, start_column: sc, end_column: ec });
        }
    }
    out.sort_by_key(|f| (f.start_line, f.start_column));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_common_secrets() {
        let text = "aws_key = AKIAQWERTYUIOPASDFGH\ntoken: ghp_abcdefghijklmnopqrstuvwxyz0123456789\nnot: xghp_abcdefghijklmnopqrstuvwxyz0123456789\n";
        let f = scan(text, false);
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(f[0].secret_type, "aws_access_key_id");
        assert_eq!((f[0].start_line, f[0].start_column, f[0].end_column), (1, 11, 31));
        assert_eq!(f[1].secret_type, "github_personal_access_token");
        assert_eq!(f[1].start_line, 2);
        let pem = "x\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAabcdefghijklmnop\n-----END RSA PRIVATE KEY-----\n";
        let f = scan(pem, false);
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].secret_type, f[0].start_line, f[0].end_line), ("rsa_private_key", 2, 4));
        let conn = "DATABASE_URL=postgres://app:hunter2@db.internal:5432/app";
        assert!(scan(conn, false).is_empty());
        assert_eq!(scan(conn, true)[0].secret_type, "postgres_connection_string");
        let stripe = format!("{}{}", "sk_live_", "4eC39HqLyjWDarjtT1zdp7dc");
        assert_eq!(scan(&stripe, false)[0].secret_type, "stripe_api_key");
        assert_eq!(scan("const k = \"AIzaSyA1234567890abcdefghijklmnopqrstuv\";", false)[0].secret_type, "google_api_key");
    }
}

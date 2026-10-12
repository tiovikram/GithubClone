//! Utility endpoints: markdown rendering, emojis, gitignore templates, licenses.
use std::collections::HashSet;
use std::sync::OnceLock;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::auth::MaybeUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/markdown", post(render))
        .route("/markdown/raw", post(render_raw))
        .route("/emojis", get(emojis))
        .route("/gitignore/templates", get(gitignore_list))
        .route("/gitignore/templates/{name}", get(gitignore_get))
        .route("/licenses", get(licenses_list))
        .route("/licenses/{license}", get(license_get))
}

// ---------------------------------------------------------------------------------------------
// Markdown

fn comrak_options(gfm: bool) -> comrak::Options<'static> {
    let mut o = comrak::Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    o.render.hardbreaks = gfm;
    o
}

/// Splits rendered HTML into (is_text, chunk) pieces, marking text that is inside
/// `<a>`, `<code>` or `<pre>` as not linkable.
fn walk_html(html: &str, mut on_text: impl FnMut(&str, &mut String), out: &mut String) {
    let mut depth_skip = 0i32;
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let end = html[i..].find('>').map(|e| i + e + 1).unwrap_or(bytes.len());
            let tag = &html[i..end];
            let lower = tag.to_ascii_lowercase();
            let name: String = lower.trim_start_matches(['<', '/']).chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if matches!(name.as_str(), "a" | "code" | "pre") {
                if lower.starts_with("</") {
                    depth_skip -= 1;
                } else if !lower.ends_with("/>") {
                    depth_skip += 1;
                }
            }
            out.push_str(tag);
            i = end;
        } else {
            let end = html[i..].find('<').map(|e| i + e).unwrap_or(bytes.len());
            let text = &html[i..end];
            if depth_skip > 0 {
                out.push_str(text);
            } else {
                on_text(text, out);
            }
            i = end;
        }
    }
}

fn is_login_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// Finds `@login` and `#123` candidates in a text chunk: (start, end, kind, value).
fn scan_refs(text: &str) -> Vec<(usize, usize, char, String)> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (idx, &(pos, c)) in chars.iter().enumerate() {
        if c != '@' && c != '#' {
            continue;
        }
        let prev_ok = idx == 0 || {
            let p = chars[idx - 1].1;
            !(p.is_alphanumeric() || p == '/' || p == '@' || p == '#' || p == '&' || p == '_')
        };
        if !prev_ok {
            continue;
        }
        let mut j = idx + 1;
        while j < chars.len() && if c == '@' { is_login_char(chars[j].1) } else { chars[j].1.is_ascii_digit() } {
            j += 1;
        }
        let end = if j < chars.len() { chars[j].0 } else { text.len() };
        let val = &text[pos + 1..end];
        if val.is_empty() || (c == '@' && (val.starts_with('-') || val.ends_with('-'))) {
            continue;
        }
        // `#1abc` or `@user.name` style continuations are not references.
        if j < chars.len() && (chars[j].1.is_alphanumeric() || chars[j].1 == '_') {
            continue;
        }
        out.push((pos, end, c, val.to_string()));
    }
    out
}

/// Renders GitHub-flavoured markdown to sanitized HTML. In `gfm` mode, `@mentions` of existing
/// users and `#N` references to existing issues in `context` (`owner/repo`) are linked.
pub async fn render_markdown(s: &AppState, text: &str, gfm: bool, context: Option<&str>) -> ApiResult<String> {
    let html = comrak::markdown_to_html(text, &comrak_options(gfm));
    if !gfm {
        return Ok(html);
    }
    let mut logins = HashSet::new();
    let mut numbers = HashSet::new();
    let mut sink = String::new();
    walk_html(
        &html,
        |t, _| {
            for (_, _, k, v) in scan_refs(t) {
                if k == '@' {
                    logins.insert(v.to_lowercase());
                } else if let Ok(n) = v.parse::<i64>() {
                    numbers.insert(n);
                }
            }
        },
        &mut sink,
    );
    let logins: Vec<String> = logins.into_iter().collect();
    let existing: HashSet<String> = if logins.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar::<_, String>("SELECT lower(login) FROM users WHERE lower(login) = ANY($1)")
            .bind(&logins)
            .fetch_all(&s.db)
            .await?
            .into_iter()
            .collect()
    };
    let mut issue_nums: HashSet<i64> = HashSet::new();
    let mut ctx_name = None;
    if let (Some(c), false) = (context, numbers.is_empty()) {
        if let Some((o, r)) = c.split_once('/') {
            if let Ok(rc) = crate::db::load_repo(&s.db, None, o, r).await {
                let nums: Vec<i64> = numbers.iter().copied().collect();
                issue_nums = sqlx::query_scalar::<_, i64>("SELECT number FROM issues WHERE repo_id = $1 AND number = ANY($2)")
                    .bind(rc.repo.id)
                    .bind(&nums)
                    .fetch_all(&s.db)
                    .await?
                    .into_iter()
                    .collect();
                ctx_name = Some(rc.full_name());
            }
        }
    }
    let base = s.base_url().to_string();
    let mut out = String::with_capacity(html.len() + 64);
    walk_html(
        &html,
        |t, out| {
            let mut last = 0;
            for (a, b, k, v) in scan_refs(t) {
                let link = if k == '@' && existing.contains(&v.to_lowercase()) {
                    Some(format!("<a class=\"user-mention notranslate\" href=\"{base}/{v}\">@{v}</a>"))
                } else if k == '#' {
                    match (&ctx_name, v.parse::<i64>()) {
                        (Some(c), Ok(n)) if issue_nums.contains(&n) => {
                            Some(format!("<a class=\"issue-link js-issue-link\" href=\"{base}/{c}/issues/{n}\">#{n}</a>"))
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(l) = link {
                    out.push_str(&t[last..a]);
                    out.push_str(&l);
                    last = b;
                }
            }
            out.push_str(&t[last..]);
        },
        &mut out,
    );
    Ok(out)
}

fn html_response(body: String) -> Response {
    let mut res = body.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html;charset=utf-8"));
    res
}

#[derive(Deserialize)]
struct RenderReq {
    text: Option<String>,
    mode: Option<String>,
    context: Option<String>,
}

async fn render(State(s): State<AppState>, _u: MaybeUser, Json(req): Json<RenderReq>) -> ApiResult<Response> {
    let text = req.text.ok_or_else(|| ApiError::validation("Markdown", "text", "missing_field"))?;
    let gfm = req.mode.as_deref() == Some("gfm");
    let html = render_markdown(&s, &text, gfm, req.context.as_deref()).await?;
    Ok(html_response(html))
}

async fn render_raw(State(s): State<AppState>, _u: MaybeUser, body: Bytes) -> ApiResult<Response> {
    let text = String::from_utf8_lossy(&body);
    let html = render_markdown(&s, &text, false, None).await?;
    Ok(html_response(html))
}

// ---------------------------------------------------------------------------------------------
// Emojis

const EMOJIS: &[(&str, &str)] = &[
    ("+1", "1f44d"), ("-1", "1f44e"), ("thumbsup", "1f44d"), ("thumbsdown", "1f44e"), ("100", "1f4af"),
    ("1234", "1f522"), ("smile", "1f604"), ("smiley", "1f603"), ("grinning", "1f600"), ("laughing", "1f606"),
    ("joy", "1f602"), ("blush", "1f60a"), ("wink", "1f609"), ("heart_eyes", "1f60d"), ("kissing_heart", "1f618"),
    ("sweat_smile", "1f605"), ("rofl", "1f923"), ("slightly_smiling_face", "1f642"), ("upside_down_face", "1f643"),
    ("innocent", "1f607"), ("thinking", "1f914"), ("neutral_face", "1f610"), ("expressionless", "1f611"),
    ("unamused", "1f612"), ("roll_eyes", "1f644"), ("grimacing", "1f62c"), ("relieved", "1f60c"), ("pensive", "1f614"),
    ("sleepy", "1f62a"), ("sleeping", "1f634"), ("mask", "1f637"), ("nerd_face", "1f913"), ("sunglasses", "1f60e"),
    ("confused", "1f615"), ("worried", "1f61f"), ("open_mouth", "1f62e"), ("astonished", "1f632"), ("flushed", "1f633"),
    ("pleading_face", "1f97a"), ("cry", "1f622"), ("sob", "1f62d"), ("scream", "1f631"), ("tired_face", "1f62b"),
    ("angry", "1f620"), ("rage", "1f621"), ("skull", "1f480"), ("poop", "1f4a9"), ("hankey", "1f4a9"), ("clown_face", "1f921"),
    ("ghost", "1f47b"), ("alien", "1f47d"), ("robot", "1f916"), ("smiley_cat", "1f63a"), ("see_no_evil", "1f648"),
    ("wave", "1f44b"), ("raised_hand", "270b"), ("ok_hand", "1f44c"), ("v", "270c"), ("crossed_fingers", "1f91e"),
    ("point_up", "261d"), ("point_right", "1f449"), ("point_left", "1f448"), ("point_down", "1f447"), ("fist", "270a"),
    ("clap", "1f44f"), ("raised_hands", "1f64c"), ("pray", "1f64f"), ("handshake", "1f91d"), ("muscle", "1f4aa"),
    ("eyes", "1f440"), ("brain", "1f9e0"), ("heart", "2764"), ("orange_heart", "1f9e1"), ("yellow_heart", "1f49b"),
    ("green_heart", "1f49a"), ("blue_heart", "1f499"), ("purple_heart", "1f49c"), ("black_heart", "1f5a4"),
    ("broken_heart", "1f494"), ("sparkling_heart", "1f496"), ("star", "2b50"), ("star2", "1f31f"), ("sparkles", "2728"),
    ("zap", "26a1"), ("fire", "1f525"), ("boom", "1f4a5"), ("collision", "1f4a5"), ("tada", "1f389"), ("hooray", "1f389"),
    ("confetti_ball", "1f38a"), ("balloon", "1f388"), ("gift", "1f381"), ("trophy", "1f3c6"), ("medal_sports", "1f3c5"),
    ("rocket", "1f680"), ("airplane", "2708"), ("car", "1f697"), ("ship", "1f6a2"), ("construction", "1f6a7"),
    ("rotating_light", "1f6a8"), ("warning", "26a0"), ("no_entry", "26d4"), ("no_entry_sign", "1f6ab"),
    ("x", "274c"), ("heavy_check_mark", "2714"), ("white_check_mark", "2705"), ("ballot_box_with_check", "2611"),
    ("heavy_plus_sign", "2795"), ("heavy_minus_sign", "2796"), ("question", "2753"), ("exclamation", "2757"),
    ("bangbang", "203c"), ("interrobang", "2049"), ("bulb", "1f4a1"), ("memo", "1f4dd"), ("pencil", "1f4dd"),
    ("pencil2", "270f"), ("book", "1f4d6"), ("books", "1f4da"), ("bookmark", "1f516"), ("label", "1f3f7"),
    ("link", "1f517"), ("paperclip", "1f4ce"), ("pushpin", "1f4cc"), ("lock", "1f512"), ("unlock", "1f513"),
    ("key", "1f511"), ("hammer", "1f528"), ("wrench", "1f527"), ("hammer_and_wrench", "1f6e0"), ("gear", "2699"),
    ("nut_and_bolt", "1f529"), ("package", "1f4e6"), ("truck", "1f69a"), ("bug", "1f41b"), ("beetle", "1fab2"),
    ("ant", "1f41c"), ("snail", "1f40c"), ("turtle", "1f422"), ("snake", "1f40d"), ("octocat", ""), ("cat", "1f431"),
    ("dog", "1f436"), ("unicorn", "1f984"), ("penguin", "1f427"), ("crab", "1f980"), ("whale", "1f433"),
    ("sunny", "2600"), ("cloud", "2601"), ("umbrella", "2614"), ("snowflake", "2744"), ("rainbow", "1f308"),
    ("earth_americas", "1f30e"), ("globe_with_meridians", "1f310"), ("moon", "1f314"), ("hourglass", "231b"),
    ("alarm_clock", "23f0"), ("stopwatch", "23f1"), ("calendar", "1f4c6"), ("date", "1f4c5"), ("chart_with_upwards_trend", "1f4c8"),
    ("chart_with_downwards_trend", "1f4c9"), ("bar_chart", "1f4ca"), ("clipboard", "1f4cb"), ("file_folder", "1f4c1"),
    ("open_file_folder", "1f4c2"), ("page_facing_up", "1f4c4"), ("mag", "1f50d"), ("mag_right", "1f50e"),
    ("computer", "1f4bb"), ("keyboard", "2328"), ("desktop_computer", "1f5a5"), ("iphone", "1f4f1"), ("bell", "1f514"),
    ("no_bell", "1f515"), ("loudspeaker", "1f4e2"), ("mega", "1f4e3"), ("speech_balloon", "1f4ac"), ("thought_balloon", "1f4ad"),
    ("email", "1f4e7"), ("envelope", "2709"), ("inbox_tray", "1f4e5"), ("outbox_tray", "1f4e4"), ("art", "1f3a8"),
    ("lipstick", "1f484"), ("recycle", "267b"), ("arrow_up", "2b06"), ("arrow_down", "2b07"), ("arrow_right", "27a1"),
    ("arrow_left", "2b05"), ("arrows_counterclockwise", "1f504"), ("rewind", "23ea"), ("fast_forward", "23e9"),
    ("twisted_rightwards_arrows", "1f500"), ("repeat", "1f501"), ("new", "1f195"), ("free", "1f193"), ("up", "1f199"),
    ("cool", "1f192"), ("ok", "1f197"), ("sos", "1f198"), ("triangular_flag_on_post", "1f6a9"), ("checkered_flag", "1f3c1"),
    ("coffee", "2615"), ("beer", "1f37a"), ("beers", "1f37b"), ("pizza", "1f355"), ("cake", "1f370"), ("apple", "1f34e"),
    ("green_apple", "1f34f"), ("lemon", "1f34b"), ("cherries", "1f352"), ("seedling", "1f331"), ("evergreen_tree", "1f332"),
    ("cactus", "1f335"), ("four_leaf_clover", "1f340"), ("rose", "1f339"), ("sunflower", "1f33b"), ("mortar_board", "1f393"),
    ("money_with_wings", "1f4b8"), ("moneybag", "1f4b0"), ("chart", "1f4b9"), ("shield", "1f6e1"), ("zzz", "1f4a4"),
    ("dash", "1f4a8"), ("sweat_drops", "1f4a6"), ("droplet", "1f4a7"), ("ocean", "1f30a"), ("volcano", "1f30b"),
    ("goal_net", "1f945"), ("dart", "1f3af"), ("game_die", "1f3b2"), ("jigsaw", "1f9e9"), ("musical_note", "1f3b5"),
    ("notes", "1f3b6"), ("headphones", "1f3a7"), ("camera", "1f4f7"), ("movie_camera", "1f3a5"), ("tv", "1f4fa"),
    ("battery", "1f50b"), ("electric_plug", "1f50c"), ("satellite", "1f4e1"), ("telescope", "1f52d"), ("microscope", "1f52c"),
    ("test_tube", "1f9ea"), ("dna", "1f9ec"), ("pill", "1f48a"), ("syringe", "1f489"), ("ambulance", "1f691"),
    ("adhesive_bandage", "1fa79"), ("lipstick_kiss", "1f48b"), ("zipper_mouth_face", "1f910"), ("shushing_face", "1f92b"),
    ("monocle_face", "1f9d0"), ("partying_face", "1f973"), ("star_struck", "1f929"), ("exploding_head", "1f92f"),
    ("hugs", "1f917"), ("facepalm", "1f926"), ("shrug", "1f937"), ("man_technologist", "1f468-1f4bb"),
    ("woman_technologist", "1f469-1f4bb"), ("technologist", "1f9d1-1f4bb"), ("busts_in_silhouette", "1f465"),
    ("bust_in_silhouette", "1f464"), ("speaking_head", "1f5e3"), ("footprints", "1f463"), ("crown", "1f451"),
    ("gem", "1f48e"), ("ring", "1f48d"), ("lock_with_ink_pen", "1f50f"), ("closed_lock_with_key", "1f510"),
    ("arrow_heading_up", "2934"), ("arrow_heading_down", "2935"), ("information_source", "2139"), ("copyright", "00a9"),
    ("registered", "00ae"), ("tm", "2122"), ("hash", "0023-20e3"), ("zero", "0030-20e3"), ("one", "0031-20e3"),
    ("two", "0032-20e3"), ("three", "0033-20e3"), ("red_circle", "1f534"), ("large_blue_circle", "1f535"),
    ("green_circle", "1f7e2"), ("yellow_circle", "1f7e1"), ("white_circle", "26aa"), ("black_circle", "26ab"),
    ("red_square", "1f7e5"), ("green_square", "1f7e9"), ("white_large_square", "2b1c"), ("black_large_square", "2b1b"),
];

fn emoji_map() -> &'static Value {
    static MAP: OnceLock<Value> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = Map::new();
        let mut names: Vec<&(&str, &str)> = EMOJIS.iter().collect();
        names.sort_by_key(|(n, _)| *n);
        for (name, cp) in names {
            let url = if cp.is_empty() {
                format!("https://github.githubassets.com/images/icons/emoji/{name}.png?v8")
            } else {
                format!("https://github.githubassets.com/images/icons/emoji/unicode/{cp}.png?v8")
            };
            m.insert(name.to_string(), json!(url));
        }
        Value::Object(m)
    })
}

async fn emojis() -> Json<Value> {
    Json(emoji_map().clone())
}

// ---------------------------------------------------------------------------------------------
// Gitignore templates

const GITIGNORE: &[(&str, &str)] = &[
    ("C", "# Prerequisites\n*.d\n\n# Object files\n*.o\n*.ko\n*.obj\n*.elf\n\n# Precompiled Headers\n*.gch\n*.pch\n\n# Libraries\n*.lib\n*.a\n*.la\n*.lo\n\n# Shared objects\n*.dll\n*.so\n*.so.*\n*.dylib\n\n# Executables\n*.exe\n*.out\n*.app\n*.i*86\n*.x86_64\n*.hex\n\n# Debug files\n*.dSYM/\n*.su\n*.idb\n*.pdb\n"),
    ("C++", "# Prerequisites\n*.d\n\n# Compiled Object files\n*.slo\n*.lo\n*.o\n*.obj\n\n# Precompiled Headers\n*.gch\n*.pch\n\n# Compiled Dynamic libraries\n*.so\n*.dylib\n*.dll\n\n# Compiled Static libraries\n*.lai\n*.la\n*.a\n*.lib\n\n# Executables\n*.exe\n*.out\n*.app\n"),
    ("Dart", "# See https://www.dartlang.org/guides/libraries/private-files\n\n# Files and directories created by pub\n.dart_tool/\n.packages\nbuild/\npubspec.lock\n"),
    ("Elixir", "/_build\n/cover\n/deps\n/doc\n/.fetch\nerl_crash.dump\n*.ez\n*.beam\n/config/*.secret.exs\n.elixir_ls/\n"),
    ("Go", "# Binaries for programs and plugins\n*.exe\n*.exe~\n*.dll\n*.so\n*.dylib\n\n# Test binary, built with `go test -c`\n*.test\n\n# Output of the go coverage tool\n*.out\n\n# Dependency directories\nvendor/\n\n# Go workspace file\ngo.work\ngo.work.sum\n\n# env file\n.env\n"),
    ("Haskell", "dist\ndist-*\ncabal-dev\n*.o\n*.hi\n*.hie\n*.chi\n*.chs.h\n*.dyn_o\n*.dyn_hi\n.hpc\n.hsenv\n.cabal-sandbox/\ncabal.sandbox.config\n*.prof\n*.aux\n*.hp\n*.eventlog\n.stack-work/\ncabal.project.local\ncabal.project.local~\n.HTF/\n.ghc.environment.*\n"),
    ("Java", "# Compiled class file\n*.class\n\n# Log file\n*.log\n\n# Package Files #\n*.jar\n*.war\n*.nar\n*.ear\n*.zip\n*.tar.gz\n*.rar\n\n# virtual machine crash logs\nhs_err_pid*\nreplay_pid*\n"),
    ("Kotlin", "# Compiled class file\n*.class\n\n# Log file\n*.log\n\n# Package Files #\n*.jar\n*.war\n*.nar\n*.ear\n*.zip\n*.tar.gz\n*.rar\n\n# virtual machine crash logs\nhs_err_pid*\nreplay_pid*\n"),
    ("Node", "# Logs\nlogs\n*.log\nnpm-debug.log*\nyarn-debug.log*\nyarn-error.log*\nlerna-debug.log*\n.pnpm-debug.log*\n\n# Runtime data\npids\n*.pid\n*.seed\n*.pid.lock\n\n# Coverage directory used by tools like istanbul\ncoverage\n*.lcov\n.nyc_output\n\n# Dependency directories\nnode_modules/\njspm_packages/\n\n# TypeScript cache\n*.tsbuildinfo\n\n# Optional npm cache directory\n.npm\n\n# Optional eslint cache\n.eslintcache\n\n# Yarn Integrity file\n.yarn-integrity\n\n# dotenv environment variable files\n.env\n.env.development.local\n.env.test.local\n.env.production.local\n.env.local\n\n# Next.js build output\n.next\nout\n\n# Nuxt.js build / generate output\n.nuxt\ndist\n\n# vuepress build output\n.vuepress/dist\n\n# Serverless directories\n.serverless/\n\n# yarn v2\n.yarn/cache\n.yarn/unplugged\n.yarn/build-state.yml\n.yarn/install-state.gz\n.pnp.*\n"),
    ("Python", "# Byte-compiled / optimized / DLL files\n__pycache__/\n*.py[cod]\n*$py.class\n\n# C extensions\n*.so\n\n# Distribution / packaging\n.Python\nbuild/\ndevelop-eggs/\ndist/\ndownloads/\neggs/\n.eggs/\nlib/\nlib64/\nparts/\nsdist/\nvar/\nwheels/\n*.egg-info/\n.installed.cfg\n*.egg\nMANIFEST\n\n# Unit test / coverage reports\nhtmlcov/\n.tox/\n.nox/\n.coverage\n.coverage.*\n.cache\nnosetests.xml\ncoverage.xml\n*.cover\n.hypothesis/\n.pytest_cache/\n\n# Jupyter Notebook\n.ipynb_checkpoints\n\n# Environments\n.env\n.venv\nenv/\nvenv/\nENV/\n\n# mypy\n.mypy_cache/\n"),
    ("Ruby", "*.gem\n*.rbc\n/.config\n/coverage/\n/InstalledFiles\n/pkg/\n/spec/reports/\n/spec/examples.txt\n/test/tmp/\n/test/version_tmp/\n/tmp/\n\n# Used by dotenv library to load environment variables.\n# .env\n\n## Documentation cache and generated files:\n/.yardoc/\n/_yardoc/\n/doc/\n/rdoc/\n\n## Environment normalization:\n/.bundle/\n/vendor/bundle\n/lib/bundler/man/\n"),
    ("Rust", "# Generated by Cargo\n# will have compiled files and executables\ndebug/\ntarget/\n\n# These are backup files generated by rustfmt\n**/*.rs.bk\n\n# MSVC Windows builds of rustc generate these, which store debugging information\n*.pdb\n"),
    ("Scala", "*.class\n*.log\n\n# virtual machine crash logs\nhs_err_pid*\n"),
    ("Swift", "# Xcode\n#\n# gitignore contributors: remember to update Global/Xcode.gitignore, Objective-C.gitignore & Swift.gitignore\n\n## User settings\nxcuserdata/\n\n## Obj-C/Swift specific\n*.hmap\n\n## App packaging\n*.ipa\n*.dSYM.zip\n*.dSYM\n\n## Playgrounds\ntimeline.xctimeline\nplayground.xcworkspace\n\n# Swift Package Manager\n.build/\n\n# CocoaPods\n# Pods/\n\n# Carthage\nCarthage/Build/\n"),
    ("Terraform", "# Local .terraform directories\n**/.terraform/*\n\n# .tfstate files\n*.tfstate\n*.tfstate.*\n\n# Crash log files\ncrash.log\ncrash.*.log\n\n# Exclude all .tfvars files\n*.tfvars\n*.tfvars.json\n\noverride.tf\noverride.tf.json\n*_override.tf\n*_override.tf.json\n\n.terraformrc\nterraform.rc\n"),
    ("Unity", "/[Ll]ibrary/\n/[Tt]emp/\n/[Oo]bj/\n/[Bb]uild/\n/[Bb]uilds/\n/[Ll]ogs/\n/[Uu]ser[Ss]ettings/\n/[Mm]emoryCaptures/\n/[Rr]ecordings/\n*.csproj\n*.unityproj\n*.sln\n*.suo\n*.tmp\n*.user\n*.userprefs\n*.pidb\n*.booproj\n*.svd\n*.pdb\n*.mdb\n*.opendb\n*.VC.db\n"),
    ("VisualStudio", "## Ignore Visual Studio temporary files, build results, and\n## files generated by popular Visual Studio add-ons.\n\n# User-specific files\n*.rsuser\n*.suo\n*.user\n*.userosscache\n*.sln.docstates\n\n# Build results\n[Dd]ebug/\n[Dd]ebugPublic/\n[Rr]elease/\n[Rr]eleases/\nx64/\nx86/\n[Ww][Ii][Nn]32/\n[Aa][Rr][Mm]/\n[Aa][Rr][Mm]64/\nbld/\n[Bb]in/\n[Oo]bj/\n[Ll]og/\n[Ll]ogs/\n\n# Visual Studio cache/options directory\n.vs/\n"),
    ("Zig", "zig-cache/\n.zig-cache/\nzig-out/\nbuild/\nbuild-*/\ndocgen_tmp/\n"),
];

/// Gitignore template content by (case-insensitive) name.
pub fn gitignore_template(name: &str) -> Option<(&'static str, &'static str)> {
    GITIGNORE.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(n, c)| (*n, *c))
}

async fn gitignore_list() -> Json<Value> {
    Json(json!(GITIGNORE.iter().map(|(n, _)| *n).collect::<Vec<_>>()))
}

async fn gitignore_get(Path(name): Path<String>) -> ApiResult<Response> {
    let (n, c) = gitignore_template(&name).ok_or_else(ApiError::not_found)?;
    Ok(axum::Json(json!({ "name": n, "source": c })).into_response())
}

// ---------------------------------------------------------------------------------------------
// Licenses (texts and metadata from choosealicense.com, MIT licensed)

pub struct License {
    pub key: &'static str,
    pub name: String,
    pub spdx_id: String,
    pub featured: bool,
    pub description: String,
    pub implementation: String,
    pub permissions: Vec<String>,
    pub conditions: Vec<String>,
    pub limitations: Vec<String>,
    pub body: String,
}

const LICENSE_FILES: &[(&str, &str)] = &[
    ("agpl-3.0", include_str!("../../assets/licenses/agpl-3.0.txt")),
    ("apache-2.0", include_str!("../../assets/licenses/apache-2.0.txt")),
    ("bsd-2-clause", include_str!("../../assets/licenses/bsd-2-clause.txt")),
    ("bsd-3-clause", include_str!("../../assets/licenses/bsd-3-clause.txt")),
    ("bsl-1.0", include_str!("../../assets/licenses/bsl-1.0.txt")),
    ("cc0-1.0", include_str!("../../assets/licenses/cc0-1.0.txt")),
    ("epl-2.0", include_str!("../../assets/licenses/epl-2.0.txt")),
    ("gpl-2.0", include_str!("../../assets/licenses/gpl-2.0.txt")),
    ("gpl-3.0", include_str!("../../assets/licenses/gpl-3.0.txt")),
    ("lgpl-2.1", include_str!("../../assets/licenses/lgpl-2.1.txt")),
    ("mit", include_str!("../../assets/licenses/mit.txt")),
    ("mpl-2.0", include_str!("../../assets/licenses/mpl-2.0.txt")),
    ("unlicense", include_str!("../../assets/licenses/unlicense.txt")),
    ("isc", include_str!("../../assets/licenses/isc.txt")),
];

/// Licenses GitHub lists as "commonly used" (ISC is available but not listed).
const COMMON: &[&str] = &[
    "agpl-3.0", "apache-2.0", "bsd-2-clause", "bsd-3-clause", "bsl-1.0", "cc0-1.0", "epl-2.0", "gpl-2.0", "gpl-3.0",
    "lgpl-2.1", "mit", "mpl-2.0", "unlicense",
];

fn parse_license(key: &'static str, text: &str) -> License {
    let text = text.replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n").unwrap_or(&text);
    let (front, body) = rest.split_once("\n---\n").unwrap_or(("", rest));
    let mut l = License {
        key,
        name: key.to_string(),
        spdx_id: key.to_uppercase(),
        featured: false,
        description: String::new(),
        implementation: String::new(),
        permissions: vec![],
        conditions: vec![],
        limitations: vec![],
        body: body.trim_start_matches('\n').to_string(),
    };
    let mut current = String::new();
    for line in front.lines() {
        if let Some(item) = line.strip_prefix("  - ") {
            let item = item.trim().to_string();
            match current.as_str() {
                "permissions" => l.permissions.push(item),
                "conditions" => l.conditions.push(item),
                "limitations" => l.limitations.push(item),
                _ => {}
            }
            continue;
        }
        if line.starts_with(' ') || line.trim().is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            current = k.trim().to_string();
            let v = v.trim().to_string();
            match current.as_str() {
                "title" => l.name = v,
                "spdx-id" => l.spdx_id = v,
                "featured" => l.featured = v == "true",
                "description" => l.description = v,
                "how" => l.implementation = v,
                _ => {}
            }
        }
    }
    l
}

pub fn licenses() -> &'static [License] {
    static L: OnceLock<Vec<License>> = OnceLock::new();
    L.get_or_init(|| LICENSE_FILES.iter().map(|(k, t)| parse_license(k, t)).collect())
}

pub fn license(key: &str) -> Option<&'static License> {
    let key = key.to_ascii_lowercase();
    licenses().iter().find(|l| l.key == key)
}

/// Node id GitHub-style for a license key.
fn license_node_id(key: &str) -> String {
    let idx = licenses().iter().position(|l| l.key == key).unwrap_or(99) + 1;
    crate::ids::encode("License", idx as i64)
}

/// `license-simple` JSON.
pub fn license_simple(api: &str, l: &License) -> Value {
    json!({
        "key": l.key,
        "name": l.name,
        "spdx_id": l.spdx_id,
        "url": format!("{api}/licenses/{}", l.key),
        "node_id": license_node_id(l.key),
    })
}

/// `license-simple` for an unrecognized license file.
pub fn license_other() -> Value {
    json!({ "key": "other", "name": "Other", "spdx_id": "NOASSERTION", "url": null, "node_id": crate::ids::encode_str("License", "other") })
}

pub fn license_full(api: &str, l: &License) -> Value {
    let mut v = license_simple(api, l);
    let o = v.as_object_mut().unwrap();
    o.insert("html_url".into(), json!(format!("http://choosealicense.com/licenses/{}/", l.key)));
    o.insert("description".into(), json!(l.description));
    o.insert("implementation".into(), json!(l.implementation));
    o.insert("permissions".into(), json!(l.permissions));
    o.insert("conditions".into(), json!(l.conditions));
    o.insert("limitations".into(), json!(l.limitations));
    o.insert("body".into(), json!(l.body));
    o.insert("featured".into(), json!(l.featured));
    v
}

/// License text with `[year]`/`[fullname]` filled in (for `license_template` on repo create).
pub fn license_text(key: &str, year: i32, fullname: &str) -> Option<String> {
    let l = license(key)?;
    Some(
        l.body
            .replace("[year]", &year.to_string())
            .replace("[yyyy]", &year.to_string())
            .replace("[fullname]", fullname)
            .replace("[name of copyright owner]", fullname),
    )
}

/// Identifies a license from file content (heuristics on distinctive phrases).
pub fn detect_license(text: &str) -> Option<&'static License> {
    let t = text.to_ascii_lowercase();
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    // GNU licenses mention each other; only their heading identifies them.
    let mut cut = t.len().min(400);
    while !t.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &t[..cut];
    let key = if t.contains("mozilla public license") && (t.contains("version 2.0") || t.contains("v. 2.0")) {
        "mpl-2.0"
    } else if t.contains("eclipse public license") && t.contains("2.0") {
        "epl-2.0"
    } else if head.contains("gnu affero general public license") {
        "agpl-3.0"
    } else if head.contains("gnu lesser general public license") {
        if t.contains("version 2.1") { "lgpl-2.1" } else { return None }
    } else if head.contains("gnu general public license") {
        if head.contains("version 3") {
            "gpl-3.0"
        } else if t.contains("version 2") {
            "gpl-2.0"
        } else {
            return None;
        }
    } else if t.contains("apache license") && t.contains("version 2.0") {
        "apache-2.0"
    } else if t.contains("boost software license") {
        "bsl-1.0"
    } else if t.contains("cc0 1.0 universal") {
        "cc0-1.0"
    } else if t.contains("this is free and unencumbered software released into the public domain") {
        "unlicense"
    } else if t.contains("permission is hereby granted, free of charge") {
        "mit"
    } else if t.contains("permission to use, copy, modify, and/or distribute this software for any purpose") {
        "isc"
    } else if t.contains("redistribution and use in source and binary forms") {
        if t.contains("neither the name") || t.contains("3. ") { "bsd-3-clause" } else { "bsd-2-clause" }
    } else {
        return None;
    };
    license(key)
}

#[derive(Deserialize)]
struct LicensesQuery {
    featured: Option<bool>,
}

async fn licenses_list(State(s): State<AppState>, Query(q): Query<LicensesQuery>) -> Json<Value> {
    let api = s.api_url();
    let items: Vec<Value> = licenses()
        .iter()
        .filter(|l| COMMON.contains(&l.key))
        .filter(|l| !q.featured.unwrap_or(false) || l.featured)
        .map(|l| license_simple(&api, l))
        .collect();
    Json(Value::Array(items))
}

async fn license_get(State(s): State<AppState>, Path(key): Path<String>) -> ApiResult<Json<Value>> {
    let l = license(&key).ok_or_else(ApiError::not_found)?;
    Ok(Json(license_full(&s.api_url(), l)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licenses_parse() {
        let mit = license("mit").unwrap();
        assert_eq!(mit.name, "MIT License");
        assert_eq!(mit.spdx_id, "MIT");
        assert!(mit.body.starts_with("MIT License"));
        assert!(mit.permissions.contains(&"commercial-use".to_string()));
        for l in licenses() {
            assert_eq!(detect_license(&l.body).map(|d| d.key), Some(l.key), "{}", l.key);
        }
    }

    #[test]
    fn refs_scan() {
        let r = scan_refs("hi @alice and #12, not a@b or #1x");
        let v: Vec<String> = r.iter().map(|x| format!("{}{}", x.2, x.3)).collect();
        assert_eq!(v, vec!["@alice", "#12"]);
    }
}

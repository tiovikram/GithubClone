//! Repository `size` (KB of packs) and primary `language` (by bytes per file extension),
//! recomputed in the background after ref changes.
use std::collections::HashMap;

use crate::state::AppState;

/// Spawns a background recomputation of size/language for `repo_id`. Never blocks the caller.
pub fn schedule_update(state: &AppState, repo_id: i64) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = update(&state, repo_id).await {
            tracing::warn!(repo_id, error = %e, "repo stats update failed");
        }
    });
}

pub async fn update(state: &AppState, repo_id: i64) -> anyhow::Result<()> {
    let path = state.git.sync(repo_id).await?;
    let git = &state.git.git;
    // Size: sum of pack sizes (what GitHub reports, in KB).
    let mut bytes: u64 = 0;
    if let Ok(mut rd) = tokio::fs::read_dir(path.join("objects/pack")).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.file_name().to_string_lossy().ends_with(".pack") {
                bytes += e.metadata().await.map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    let size_kb = bytes.div_ceil(1024) as i64;
    let (head, refs) = state.git.refs(repo_id).await?;
    let language = if refs.contains_key(&head) {
        let out = git.run(&path, &["ls-tree", "-r", "-l", "-z", "--full-tree", &head]).await?;
        primary_language(super::read::parse_ls_tree(&out).iter().filter_map(|e| Some((e.path.as_str(), e.size?))))
    } else {
        None
    };
    sqlx::query("UPDATE repositories SET size_kb = $2, language = $3 WHERE id = $1")
        .bind(repo_id)
        .bind(size_kb)
        .bind(&language)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Bytes per language for (path, size) pairs, ignoring vendored/unknown files.
pub fn language_bytes<'a>(files: impl Iterator<Item = (&'a str, u64)>) -> HashMap<&'static str, u64> {
    let mut by: HashMap<&'static str, u64> = HashMap::new();
    for (path, size) in files {
        if path.starts_with("vendor/") || path.contains("node_modules/") || path.contains("/vendor/") {
            continue;
        }
        if let Some(lang) = language_for(path) {
            *by.entry(lang).or_default() += size;
        }
    }
    by
}

pub fn primary_language<'a>(files: impl Iterator<Item = (&'a str, u64)>) -> Option<String> {
    language_bytes(files)
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(l, _)| l.to_string())
}

/// GitHub (linguist) language name for a path, for common programming languages only
/// (prose/data formats such as Markdown, JSON, YAML don't count, like on GitHub).
pub fn language_for(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name {
        "Dockerfile" => return Some("Dockerfile"),
        "Makefile" | "GNUmakefile" => return Some("Makefile"),
        _ => {}
    }
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "Rust",
        "go" => "Go",
        "py" | "pyw" => "Python",
        "js" | "mjs" | "cjs" | "jsx" => "JavaScript",
        "ts" | "tsx" | "mts" | "cts" => "TypeScript",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "scala" => "Scala",
        "rb" => "Ruby",
        "php" => "PHP",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "C++",
        "cs" => "C#",
        "fs" | "fsx" => "F#",
        "m" => "Objective-C",
        "mm" => "Objective-C++",
        "swift" => "Swift",
        "dart" => "Dart",
        "lua" => "Lua",
        "pl" | "pm" => "Perl",
        "r" => "R",
        "jl" => "Julia",
        "hs" => "Haskell",
        "ml" | "mli" => "OCaml",
        "ex" | "exs" => "Elixir",
        "erl" | "hrl" => "Erlang",
        "clj" | "cljs" | "cljc" => "Clojure",
        "elm" => "Elm",
        "zig" => "Zig",
        "nim" => "Nim",
        "v" | "sv" => "Verilog",
        "vhd" | "vhdl" => "VHDL",
        "sh" | "bash" | "zsh" => "Shell",
        "ps1" | "psm1" => "PowerShell",
        "bat" | "cmd" => "Batchfile",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" => "SCSS",
        "sass" => "Sass",
        "less" => "Less",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "sql" => "PLpgSQL",
        "tf" | "hcl" => "HCL",
        "nix" => "Nix",
        "groovy" | "gradle" => "Groovy",
        "asm" | "s" => "Assembly",
        "tex" => "TeX",
        "vim" => "Vim Script",
        "cmake" => "CMake",
        "proto" => "Protocol Buffer",
        "sol" => "Solidity",
        "ipynb" => "Jupyter Notebook",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn picks_largest() {
        let files = vec![("src/a.rs", 100u64), ("b.py", 50), ("README.md", 1000), ("c.py", 60)];
        assert_eq!(super::primary_language(files.into_iter()).as_deref(), Some("Python"));
        assert_eq!(super::primary_language(vec![("README.md", 10u64)].into_iter()), None);
    }
}

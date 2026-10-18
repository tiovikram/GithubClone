//! Workflow files: YAML -> JSON, trigger matching and job planning (matrix expansion).
use serde_json::{json, Map, Value};
use yaml_rust2::{Yaml, YamlLoader};

use super::expr::{self, Ctx};
use super::glob::matches_filters;

fn yaml_key(y: &Yaml) -> String {
    match y {
        Yaml::String(s) | Yaml::Real(s) => s.clone(),
        Yaml::Integer(i) => i.to_string(),
        Yaml::Boolean(b) => b.to_string(),
        Yaml::Null => "null".into(),
        _ => String::new(),
    }
}

fn yaml_to_json(y: &Yaml) -> Value {
    match y {
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Integer(i) => json!(i),
        Yaml::Real(r) => r.parse::<f64>().map(|f| json!(f)).unwrap_or_else(|_| Value::String(r.clone())),
        Yaml::Boolean(b) => Value::Bool(*b),
        Yaml::Array(a) => Value::Array(a.iter().map(yaml_to_json).collect()),
        Yaml::Hash(h) => {
            let mut m = Map::new();
            for (k, v) in h {
                m.insert(yaml_key(k), yaml_to_json(v));
            }
            Value::Object(m)
        }
        _ => Value::Null,
    }
}

/// Parses YAML text into JSON (first document).
pub fn parse_yaml(text: &str) -> Result<Value, String> {
    let docs = YamlLoader::load_from_str(text).map_err(|e| e.to_string())?;
    Ok(docs.first().map(yaml_to_json).unwrap_or(Value::Null))
}

/// A parsed workflow file.
#[derive(Debug, Clone)]
pub struct Workflow {
    pub raw: Value,
    pub name: Option<String>,
    /// event name -> configuration (object or null)
    pub on: Map<String, Value>,
    /// (job key, job definition) in file order
    pub jobs: Vec<(String, Value)>,
}

pub fn parse(text: &str) -> Result<Workflow, String> {
    from_value(parse_yaml(text)?)
}

/// Builds a workflow from its JSON form (as stored with a run).
pub fn from_value(raw: Value) -> Result<Workflow, String> {
    let obj = raw.as_object().ok_or("workflow is not a mapping")?;
    let on_v = obj.get("on").or_else(|| obj.get("true")).cloned().unwrap_or(Value::Null);
    let mut on = Map::new();
    match on_v {
        Value::String(s) => {
            on.insert(s, Value::Null);
        }
        Value::Array(a) => {
            for e in a {
                if let Value::String(s) = e {
                    on.insert(s, Value::Null);
                }
            }
        }
        Value::Object(m) => on = m,
        _ => return Err("workflow has no 'on' trigger".into()),
    }
    let jobs: Vec<(String, Value)> = match obj.get("jobs") {
        Some(Value::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => vec![],
    };
    if jobs.is_empty() {
        return Err("workflow must define at least one job".into());
    }
    for (k, j) in &jobs {
        if !j.is_object() {
            return Err(format!("job '{k}' is not a mapping"));
        }
        if j.get("uses").is_some() {
            return Err(format!("job '{k}': reusable workflows (jobs.<id>.uses) are not supported"));
        }
        if j.get("steps").and_then(Value::as_array).is_none_or(|s| s.is_empty()) {
            return Err(format!("job '{k}' has no steps"));
        }
        for n in needs_of(j) {
            if !jobs.iter().any(|(kk, _)| *kk == n) {
                return Err(format!("job '{k}' depends on unknown job '{n}'"));
            }
        }
    }
    // dependency cycles
    let mut done: Vec<String> = vec![];
    while done.len() < jobs.len() {
        let before = done.len();
        for (k, j) in &jobs {
            if !done.contains(k) && needs_of(j).iter().all(|n| done.contains(n)) {
                done.push(k.clone());
            }
        }
        if done.len() == before {
            return Err("the workflow's jobs have a dependency cycle".into());
        }
    }
    let name = obj.get("name").and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty());
    Ok(Workflow { raw, name, on, jobs })
}

/// Lenient workflow name used when listing workflows (even invalid files have a name).
pub fn display_name(text: &str, path: &str) -> String {
    parse_yaml(text)
        .ok()
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| path.to_string())
}

pub fn needs_of(job: &Value) -> Vec<String> {
    match job.get("needs") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => vec![],
    }
}

fn str_list(v: Option<&Value>) -> Option<Vec<String>> {
    match v? {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(a) => Some(a.iter().map(expr::to_string).collect()),
        _ => None,
    }
}

/// What happened, for trigger matching.
pub struct EventInfo<'a> {
    pub event: &'a str,
    /// payload `action` (opened, synchronize, ...)
    pub action: Option<&'a str>,
    /// push: full ref; pull_request: base branch ref
    pub git_ref: Option<&'a str>,
    /// changed paths, when known (paths filters)
    pub paths: Option<&'a [String]>,
}

fn default_types(event: &str) -> Option<&'static [&'static str]> {
    match event {
        "pull_request" | "pull_request_target" => Some(&["opened", "synchronize", "reopened"]),
        _ => None,
    }
}

fn paths_ok(cfg: &Value, paths: Option<&[String]>) -> bool {
    let Some(paths) = paths else { return true };
    if let Some(p) = str_list(cfg.get("paths")) {
        if !paths.iter().any(|f| matches_filters(&p, f)) {
            return false;
        }
    }
    if let Some(p) = str_list(cfg.get("paths-ignore")) {
        if !paths.is_empty() && paths.iter().all(|f| matches_filters(&p, f)) {
            return false;
        }
    }
    true
}

impl Workflow {
    /// Whether the workflow runs for this event.
    pub fn triggered_by(&self, ev: &EventInfo) -> bool {
        let Some(cfg) = self.on.get(ev.event) else { return false };
        let cfg = if cfg.is_object() { cfg.clone() } else { json!({}) };
        // activity types
        let types = str_list(cfg.get("types"));
        match (&types, ev.action) {
            (Some(t), Some(a)) => {
                if !t.iter().any(|x| x == a) {
                    return false;
                }
            }
            (None, Some(a)) => {
                if let Some(d) = default_types(ev.event) {
                    if !d.contains(&a) {
                        return false;
                    }
                }
            }
            _ => {}
        }
        match ev.event {
            "push" => {
                let r = ev.git_ref.unwrap_or("");
                let branches = str_list(cfg.get("branches"));
                let branches_ignore = str_list(cfg.get("branches-ignore"));
                let tags = str_list(cfg.get("tags"));
                let tags_ignore = str_list(cfg.get("tags-ignore"));
                let any_branch_filter = branches.is_some() || branches_ignore.is_some();
                let any_tag_filter = tags.is_some() || tags_ignore.is_some();
                if let Some(b) = r.strip_prefix("refs/heads/") {
                    if !any_branch_filter && any_tag_filter {
                        return false;
                    }
                    if let Some(f) = &branches {
                        if !matches_filters(f, b) {
                            return false;
                        }
                    }
                    if let Some(f) = &branches_ignore {
                        if matches_filters(f, b) {
                            return false;
                        }
                    }
                } else if let Some(t) = r.strip_prefix("refs/tags/") {
                    if !any_tag_filter && any_branch_filter {
                        return false;
                    }
                    if let Some(f) = &tags {
                        if !matches_filters(f, t) {
                            return false;
                        }
                    }
                    if let Some(f) = &tags_ignore {
                        if matches_filters(f, t) {
                            return false;
                        }
                    }
                    // paths filters are not evaluated for tag pushes
                    return true;
                }
                paths_ok(&cfg, ev.paths)
            }
            "pull_request" | "pull_request_target" => {
                let b = ev.git_ref.unwrap_or("").trim_start_matches("refs/heads/");
                if let Some(f) = str_list(cfg.get("branches")) {
                    if !matches_filters(&f, b) {
                        return false;
                    }
                }
                if let Some(f) = str_list(cfg.get("branches-ignore")) {
                    if matches_filters(&f, b) {
                        return false;
                    }
                }
                paths_ok(&cfg, ev.paths)
            }
            _ => true,
        }
    }

    pub fn crons(&self) -> Vec<String> {
        match self.on.get("schedule") {
            Some(Value::Array(a)) => a.iter().filter_map(|e| e.get("cron").and_then(Value::as_str).map(str::to_string)).collect(),
            _ => vec![],
        }
    }

    pub fn dispatch_inputs(&self) -> Map<String, Value> {
        self.on.get("workflow_dispatch").and_then(|c| c.get("inputs")).and_then(Value::as_object).cloned().unwrap_or_default()
    }
}

/// A job instance after matrix expansion.
#[derive(Debug, Clone)]
pub struct PlannedJob {
    pub name: String,
    pub matrix: Option<Value>,
    pub labels: Vec<String>,
}

/// Cartesian product of a matrix definition with `include`/`exclude` applied (GitHub rules).
pub fn expand_matrix(matrix: &Value) -> Result<Vec<Map<String, Value>>, String> {
    let Some(m) = matrix.as_object() else {
        return Err("strategy.matrix must be a mapping".into());
    };
    let mut combos: Vec<Map<String, Value>> = vec![Map::new()];
    let mut dims = 0;
    for (k, v) in m {
        if k == "include" || k == "exclude" {
            continue;
        }
        let vals = match v {
            Value::Array(a) => a.clone(),
            other => return Err(format!("matrix dimension '{k}' must be a list, got {other}")),
        };
        dims += 1;
        let mut next = Vec::new();
        for c in &combos {
            for x in &vals {
                let mut n = c.clone();
                n.insert(k.clone(), x.clone());
                next.push(n);
            }
        }
        combos = next;
    }
    if dims == 0 {
        combos.clear();
    }
    let partial_match = |combo: &Map<String, Value>, pat: &Map<String, Value>| pat.iter().all(|(k, v)| combo.get(k) == Some(v));
    if let Some(Value::Array(ex)) = m.get("exclude") {
        for e in ex.iter().filter_map(Value::as_object) {
            combos.retain(|c| !partial_match(c, e));
        }
    }
    let originals: Vec<Map<String, Value>> = combos.clone();
    if let Some(Value::Array(inc)) = m.get("include") {
        for e in inc.iter().filter_map(Value::as_object) {
            let mut extended = false;
            for (i, c) in combos.iter_mut().enumerate() {
                let orig = &originals[i];
                // can extend if it overwrites none of the original matrix values
                let compatible = e.iter().all(|(k, v)| orig.get(k).is_none_or(|ov| ov == v));
                if compatible {
                    for (k, v) in e {
                        c.insert(k.clone(), v.clone());
                    }
                    extended = true;
                }
            }
            if !extended {
                combos.push(e.clone());
            }
        }
    }
    if combos.len() > 256 {
        return Err("a matrix may generate at most 256 jobs".into());
    }
    Ok(combos)
}

/// `runs-on` as a list of labels.
pub fn runs_on_labels(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().map(expr::to_string).collect(),
        Value::Object(o) => str_list(o.get("labels")).unwrap_or_default(),
        _ => vec![],
    }
}

/// Expands one job of the workflow into its instances. `contexts` provides `github`, `needs`,
/// `inputs`, `vars` for expressions in `strategy`, `runs-on` and `name`.
pub fn plan_job(key: &str, job: &Value, contexts: &Value) -> Result<Vec<PlannedJob>, String> {
    let ctx = Ctx::new(contexts);
    let matrix_def = job.pointer("/strategy/matrix").map(|m| expr::interpolate_value(&ctx, m)).transpose()?;
    let combos: Vec<Option<Map<String, Value>>> = match &matrix_def {
        Some(Value::String(s)) => return Err(format!("strategy.matrix evaluated to a string: {s}")),
        Some(m) => {
            let c = expand_matrix(m)?;
            if c.is_empty() {
                return Err(format!("job '{key}': matrix produced no jobs"));
            }
            c.into_iter().map(Some).collect()
        }
        None => vec![None],
    };
    let mut out = Vec::new();
    for combo in combos {
        let mut c = contexts.clone();
        if let Some(m) = &combo {
            c["matrix"] = Value::Object(m.clone());
        }
        let jctx = Ctx::new(&c);
        let base_name = job.get("name").map(|n| expr::interpolate(&jctx, &expr::to_string(n))).transpose()?;
        let name = match (&base_name, &combo) {
            (Some(n), Some(m)) if !job.get("name").map(expr::to_string).unwrap_or_default().contains("matrix") && !m.is_empty() => {
                format!("{n} ({})", m.values().map(matrix_display).collect::<Vec<_>>().join(", "))
            }
            (Some(n), _) => n.clone(),
            (None, Some(m)) if !m.is_empty() => format!("{key} ({})", m.values().map(matrix_display).collect::<Vec<_>>().join(", ")),
            (None, _) => key.to_string(),
        };
        let runs_on = job.get("runs-on").map(|r| expr::interpolate_value(&jctx, r)).transpose()?.unwrap_or(Value::Null);
        out.push(PlannedJob { name, matrix: combo.map(Value::Object), labels: runs_on_labels(&runs_on) });
    }
    Ok(out)
}

fn matrix_display(v: &Value) -> String {
    match v {
        Value::Object(_) | Value::Array(_) => serde_json::to_string(v).unwrap_or_default(),
        other => expr::to_string(other),
    }
}

/// Labels that name GitHub-hosted images. forge has no hosted runners, so these labels match
/// any runner (the label set of the job is otherwise matched exactly).
pub fn is_hosted_label(l: &str) -> bool {
    let l = l.to_ascii_lowercase();
    l.starts_with("ubuntu-") || l.starts_with("macos-") || l.starts_with("windows-") || l == "ubuntu" || l == "github-hosted"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_trigger() {
        let w = parse("name: CI\non:\n  push:\n    branches: [main]\n    paths-ignore: ['docs/**']\n  pull_request:\njobs:\n  b:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n").unwrap();
        assert_eq!(w.name.as_deref(), Some("CI"));
        let paths = vec!["src/a.rs".to_string()];
        let docs = vec!["docs/a.md".to_string()];
        assert!(w.triggered_by(&EventInfo { event: "push", action: None, git_ref: Some("refs/heads/main"), paths: Some(&paths) }));
        assert!(!w.triggered_by(&EventInfo { event: "push", action: None, git_ref: Some("refs/heads/main"), paths: Some(&docs) }));
        assert!(!w.triggered_by(&EventInfo { event: "push", action: None, git_ref: Some("refs/heads/dev"), paths: None }));
        assert!(!w.triggered_by(&EventInfo { event: "push", action: None, git_ref: Some("refs/tags/v1"), paths: None }));
        assert!(w.triggered_by(&EventInfo { event: "pull_request", action: Some("opened"), git_ref: Some("main"), paths: None }));
        assert!(!w.triggered_by(&EventInfo { event: "pull_request", action: Some("closed"), git_ref: Some("main"), paths: None }));
        assert!(parse("on: push\njobs:\n  a:\n    needs: b\n    steps: [{run: x}]\n").is_err());
        let w = parse("on: [push, workflow_dispatch]\njobs:\n  a:\n    steps: [{run: x}]\n").unwrap();
        assert!(w.on.contains_key("workflow_dispatch"));
    }

    #[test]
    fn matrix() {
        let m = json!({"os": ["linux", "mac"], "v": [1, 2], "exclude": [{"os": "mac", "v": 1}], "include": [{"os": "linux", "extra": true}, {"os": "win", "v": 9}]});
        let c = expand_matrix(&m).unwrap();
        assert_eq!(c.len(), 4);
        assert_eq!(c.iter().filter(|x| x.get("extra").is_some()).count(), 2);
        let jobs = plan_job("test", &json!({"runs-on": "${{ matrix.os }}", "strategy": {"matrix": {"os": ["a", "b"]}}}), &json!({})).unwrap();
        assert_eq!(jobs[1].name, "test (b)");
        assert_eq!(jobs[1].labels, vec!["b"]);
    }
}

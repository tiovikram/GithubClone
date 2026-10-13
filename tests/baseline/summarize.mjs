#!/usr/bin/env node
// Builds tests/baseline/results/summary.md from the raw k6 JSON summaries (median over runs)
// and the compat reports.   node tests/baseline/summarize.mjs [label ...]
// Default labels: forgejo-1 forge-1 forge-3 (columns, in that order).
import fs from "node:fs";
import path from "node:path";

const HERE = path.dirname(new URL(import.meta.url).pathname);
const RES = path.join(HERE, "results");
const LABELS = process.argv.slice(2).length ? process.argv.slice(2) : ["forgejo-1", "forge-1", "forge-3"];
const SCEN = ["read-r100", "read-r500", "read-r1000", "read-sat", "git-c1", "git-c8", "git-c32", "write-r10", "write-r25", "write-r50"];

const median = (xs) => {
  const v = xs.filter((x) => x !== undefined && x !== null && !Number.isNaN(x)).sort((a, b) => a - b);
  if (!v.length) return undefined;
  const m = Math.floor(v.length / 2);
  return v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2;
};
const f1 = (x) => (x === undefined ? "–" : x >= 100 ? x.toFixed(0) : x.toFixed(1));
const pct = (x) => (x === undefined ? "–" : `${(x * 100).toFixed(x > 0 && x < 0.001 ? 3 : 2)}%`);

function runs(label, s) {
  const dir = path.join(RES, "raw", label);
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir)
    .filter((f) => f.startsWith(`${s}-run`) && f.endsWith(".json"))
    .map((f) => JSON.parse(fs.readFileSync(path.join(dir, f), "utf8")).metrics);
}
function stat(label, s) {
  const rs = runs(label, s);
  if (!rs.length) return null;
  const g = (fn) => median(rs.map((m) => { try { return fn(m); } catch { return undefined; } }));
  const git = s.startsWith("git");
  return {
    n: rs.length,
    rps: g((m) => m.http_reqs.values.rate),
    ops: g((m) => m.iterations.values.rate),
    p50: g((m) => m.http_req_duration.values.med),
    p95: g((m) => m.http_req_duration.values["p(95)"]),
    p99: g((m) => m.http_req_duration.values["p(99)"]),
    fail: g((m) => m.http_req_failed.values.rate),
    dropped: g((m) => (m.dropped_iterations ? m.dropped_iterations.values.count : 0)),
    clone50: git ? g((m) => m.git_clone_duration.values.med) : undefined,
    clone95: git ? g((m) => m.git_clone_duration.values["p(95)"]) : undefined,
    clone99: git ? g((m) => m.git_clone_duration.values["p(99)"]) : undefined,
    perName: (n, k) => g((m) => m[`http_req_duration{name:${n}}`].values[k]),
  };
}

const out = [];
const p = (s = "") => out.push(s);
const DESC = {
  "read-r100": "API reads @ 100 req/s", "read-r500": "API reads @ 500 req/s", "read-r1000": "API reads @ 1000 req/s",
  "read-sat": "API reads, saturation (64 VUs closed loop)",
  "git-c1": "git clone/fetch, 1 VU", "git-c8": "git clone/fetch, 8 VUs", "git-c32": "git clone/fetch, 32 VUs",
  "write-r10": "write mix @ 10 it/s (20 req/s)", "write-r25": "write mix @ 25 it/s (50 req/s)", "write-r50": "write mix @ 50 it/s (100 req/s)",
};

p("## Performance (median of runs)");
p();
p("Latency is k6 `http_req_duration` over all requests of the scenario, in ms. *Throughput* is completed requests/s;");
p("*dropped* = iterations k6 could not start because all VUs were busy waiting (open-model overload signal).");
p();
p(`| scenario | metric | ${LABELS.join(" | ")} |`);
p(`|---|---|${LABELS.map(() => "---:").join("|")}|`);
for (const s of SCEN) {
  const st = LABELS.map((l) => stat(l, s));
  if (st.every((x) => !x)) continue;
  const row = (name, fn) => p(`| ${DESC[s]} | ${name} | ${st.map((x) => (x ? fn(x) : "–")).join(" | ")} |`);
  row("throughput req/s", (x) => f1(x.rps));
  if (s.startsWith("git")) row("clones+fetches/s", (x) => f1(x.ops));
  row("p50 / p95 / p99 ms", (x) => `${f1(x.p50)} / ${f1(x.p95)} / ${f1(x.p99)}`);
  if (s.startsWith("git")) row("full sequence p50 / p95 / p99 ms", (x) => `${f1(x.clone50)} / ${f1(x.clone95)} / ${f1(x.clone99)}`);
  row("errors", (x) => pct(x.fail));
  if (!s.startsWith("git") && s !== "read-sat") row("dropped iterations", (x) => f1(x.dropped));
}
p();
p("### Per-endpoint p95 (ms), API reads @ 500 req/s and saturation");
p();
p(`| endpoint | ${LABELS.map((l) => `${l} @500`).join(" | ")} | ${LABELS.map((l) => `${l} sat`).join(" | ")} |`);
p(`|---|${LABELS.concat(LABELS).map(() => "---:").join("|")}|`);
for (const n of ["get_repo", "list_issues", "get_contents", "get_user", "list_user_repos"]) {
  const a = LABELS.map((l) => stat(l, "read-r500")).map((x) => (x ? f1(x.perName(n, "p(95)")) : "–"));
  const b = LABELS.map((l) => stat(l, "read-sat")).map((x) => (x ? f1(x.perName(n, "p(95)")) : "–"));
  p(`| ${n} | ${a.join(" | ")} | ${b.join(" | ")} |`);
}
p();
p("### Write mix per request p95 (ms)");
p();
p(`| rate | request | ${LABELS.join(" | ")} |`);
p(`|---|---|${LABELS.map(() => "---:").join("|")}|`);
for (const s of ["write-r10", "write-r25", "write-r50"]) for (const n of ["create_issue", "create_comment"]) {
  p(`| ${s.slice(7)} it/s | ${n} | ${LABELS.map((l) => stat(l, s)).map((x) => (x ? f1(x.perName(n, "p(95)")) : "–")).join(" | ")} |`);
}
p();
p(`Runs per scenario: ${LABELS.map((l) => `${l}: ${Math.min(...SCEN.map((s) => runs(l, s).length))}`).join(", ")}.`);

// ---- compat
const C = path.join(RES, "compat");
const cj = (n) => { try { return JSON.parse(fs.readFileSync(path.join(C, `conformance-${n}.json`), "utf8")).summary; } catch { return null; } };
const conf = { "forge /api/v3": cj("forge-v3"), "Forgejo /api/v1": cj("forgejo-v1"), "Forgejo /api/v3": cj("forgejo-v3") };
p();
p("## REST conformance (tools/conformance, GHES 3.22 spec)");
p();
p("| target | overall | tier 1 | tier 2 | tested ops | nonconforming | not implemented |");
p("|---|---:|---:|---:|---:|---:|---:|");
for (const [k, v] of Object.entries(conf)) {
  if (!v) continue;
  const c = (x) => `${x.conforming}/${x.total} (${x.pct_conforming}%)`;
  p(`| ${k} | ${c(v.overall)} | ${c(v.tier1)} | ${c(v.tier2)} | ${v.overall.tested} | ${v.overall.nonconforming} | ${v.overall.not_implemented} |`);
}
p();
p("Per tag (tags where any target was tested): conforming / ops (% of tested)");
p();
p(`| tag | ops | ${Object.keys(conf).join(" | ")} |`);
p(`|---|---:|${Object.keys(conf).map(() => "---:").join("|")}|`);
const tags = Object.keys(conf["forge /api/v3"]?.byTag || {});
for (const t of tags) {
  const vals = Object.values(conf).map((v) => v?.byTag?.[t]);
  if (vals.every((x) => !x || !x.tested)) continue;
  p(`| ${t} | ${vals[0].total} | ${vals.map((x) => (!x ? "–" : x.tested ? `${x.conforming} (${x.pct_conforming_of_tested}%)` : "0 (untested)")).join(" | ")} |`);
}
const ghPass = (n) => { try { return /\*\*PASS (\d+)\/(\d+)/.exec(fs.readFileSync(path.join(C, `gh-${n}.md`), "utf8")).slice(1, 3).join("/"); } catch { return "–"; } };
const gitPass = (n) => { try { return /PASS (\d+\/\d+)/.exec(fs.readFileSync(path.join(C, `git-${n}.txt`), "utf8").split("\n").filter((l) => l.includes("smoke:")).join(""))[1]; } catch { return "–"; } };
p();
p("## gh CLI suite and stock git");
p();
p("| check | forge | Forgejo |");
p("|---|---:|---:|");
p(`| tests/gh/run.sh (gh as enterprise host) | ${ghPass("forge")} | ${ghPass("forgejo")} |`);
p(`| git smart HTTP smoke (tests/baseline/git-smoke.sh) | ${gitPass("forge")} | ${gitPass("forgejo")} |`);

fs.writeFileSync(path.join(RES, "summary.md"), "# Baseline summary (generated by tests/baseline/summarize.mjs)\n\n" + out.join("\n") + "\n");
console.log(out.join("\n"));

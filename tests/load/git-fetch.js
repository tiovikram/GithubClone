// (b) git smart HTTP load: what `git clone` does with protocol v2, replayed request-by-request:
//   1. GET  /{o}/{r}.git/info/refs?service=git-upload-pack   (Git-Protocol: version=2)
//   2. POST /{o}/{r}.git/git-upload-pack  command=ls-refs     -> HEAD oid
//   3. POST /{o}/{r}.git/git-upload-pack  command=fetch, want <oid>, done -> packfile
// The request bodies are the ones stock git 2.50 sends (captured with GIT_TRACE_PACKET=1, see
// tests/load/fixtures/clone-v2.trace); only the want oid is substituted. A fraction of the
// iterations do an incremental fetch (want HEAD, have HEAD~) like `git fetch` after one commit.
//   k6 run -e BASE_URL=... -e TOKEN=... -e REPOS=admin/loadfix tests/load/git-fetch.js
import http from "k6/http";
import { check } from "k6";
import { Trend, Counter } from "k6/metrics";
import { BASE, basicAuth, fixtureRepos, pick, summaryHandler, TREND_STATS, HTTP_TIMEOUT } from "./lib.js";

const VUS = Number(__ENV.VUS || 10);
const DURATION = __ENV.DURATION || "60s";

export const options = {
  summaryTrendStats: TREND_STATS,
  scenarios: {
    clone: { executor: "constant-vus", vus: VUS, duration: DURATION },
  },
  thresholds: {
    http_req_failed: ["rate<0.01"],
    "http_req_duration{step:info_refs}": ["p(95)<300"],
    "http_req_duration{step:ls_refs}": ["p(95)<300"],
    "http_req_duration{step:fetch}": ["p(95)<1500"],
    git_clone_duration: ["p(95)<2000"],
    checks: ["rate>0.99"],
  },
};

const cloneTime = new Trend("git_clone_duration", true);
const packBytes = new Counter("git_pack_bytes");

function pkt(s) {
  const len = (s.length + 4).toString(16).padStart(4, "0");
  return len + s;
}
const FLUSH = "0000";
const DELIM = "0001";
const AGENT = "agent=git/2.50.1-k6";

function lsRefsBody() {
  return (
    pkt("command=ls-refs\n") + pkt(AGENT + "\n") + pkt("object-format=sha1\n") + DELIM +
    pkt("peel\n") + pkt("symrefs\n") + pkt("unborn\n") +
    pkt("ref-prefix refs/heads/\n") + pkt("ref-prefix refs/tags/\n") + pkt("ref-prefix HEAD\n") + FLUSH
  );
}

function fetchBody(wants, haves) {
  let b = pkt("command=fetch\n") + pkt(AGENT + "\n") + pkt("object-format=sha1\n") + DELIM +
    pkt("thin-pack\n") + pkt("no-progress\n") + pkt("ofs-delta\n");
  for (const w of wants) b += pkt(`want ${w}\n`);
  for (const h of haves) b += pkt(`have ${h}\n`);
  return b + pkt("done\n") + FLUSH;
}

function params(step, extra = {}) {
  return {
    headers: {
      Authorization: basicAuth(),
      "Git-Protocol": "version=2",
      "User-Agent": "git/2.50.1-k6",
      ...extra,
    },
    tags: { step },
    timeout: HTTP_TIMEOUT,
  };
}

export default function () {
  const repo = pick(fixtureRepos());
  const url = `${BASE}/${repo}.git`;
  const t0 = Date.now();

  const adv = http.get(`${url}/info/refs?service=git-upload-pack`, params("info_refs"));
  check(adv, {
    "info/refs 200": (r) => r.status === 200,
    "v2 advertisement": (r) => typeof r.body === "string" && r.body.includes("version 2") && r.body.includes("fetch"),
  });

  const post = { "Content-Type": "application/x-git-upload-pack-request", Accept: "application/x-git-upload-pack-result" };
  const ls = http.post(`${url}/git-upload-pack`, lsRefsBody(), params("ls_refs", post));
  const okLs = check(ls, { "ls-refs 200": (r) => r.status === 200 });
  if (!okLs) return;
  const m = /([0-9a-f]{40}) HEAD/.exec(ls.body || "");
  if (!check(m, { "ls-refs has HEAD": (x) => x !== null })) return;
  const head = m[1];

  // 80% full clones, 20% "fetch one new commit" style negotiations (have = parent unknown to
  // us, so we send the HEAD itself as have and get an empty/near-empty pack).
  const incremental = Math.random() < 0.2;
  const body = incremental ? fetchBody([head], [head]) : fetchBody([head], []);
  const f = http.post(`${url}/git-upload-pack`, body, params("fetch", post));
  check(f, {
    "fetch 200": (r) => r.status === 200,
    "fetch has packfile": (r) => typeof r.body === "string" && r.body.includes("packfile") && r.body.includes("PACK"),
  });
  packBytes.add(f.body ? f.body.length : 0);
  cloneTime.add(Date.now() - t0, { incremental: String(incremental) });
}

export const handleSummary = summaryHandler;

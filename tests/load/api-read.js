// (a) REST API read mix. Core endpoints (exist on main) must succeed; "optional" endpoints owned
// by other areas (issues, contents, branches, ...) may 404 until merged and are tolerated, but
// are still measured so the mix grows automatically as they land.
//   k6 run -e BASE_URL=http://localhost:8080 -e TOKEN=... -e REPOS=admin/loadfix tests/load/api-read.js
import http from "k6/http";
import { check } from "k6";
import { API, OWNER, apiParams, fixtureRepos, pick, optionalOk, summaryHandler, TREND_STATS } from "./lib.js";

const VUS = Number(__ENV.VUS || 20);
const DURATION = __ENV.DURATION || "60s";

export const options = {
  summaryTrendStats: TREND_STATS,
  scenarios: {
    read: { executor: "constant-vus", vus: VUS, duration: DURATION },
  },
  thresholds: {
    "http_req_failed{kind:core}": ["rate<0.01"],
    "http_req_duration{kind:core}": ["p(95)<250", "p(99)<600"],
    "http_req_duration{kind:optional}": ["p(95)<800"],
    checks: ["rate>0.99"],
  },
};

// [weight, name, path(repo), kind]
const CORE = [
  [4, "GET /repos/{o}/{r}", (r) => `/repos/${r}`],
  [2, "GET /user", () => `/user`],
  [2, "GET /user/repos", () => `/user/repos?per_page=30`],
  [2, "GET /users/{u}", () => `/users/${OWNER}`],
  [1, "GET /users/{u}/repos", () => `/users/${OWNER}/repos`],
  [1, "GET /repos/{o}/{r}/languages", (r) => `/repos/${r}/languages`],
  [1, "GET /rate_limit", () => `/rate_limit`],
];
const OPTIONAL = [
  [2, "GET /repos/{o}/{r}/issues", (r) => `/repos/${r}/issues?state=all&per_page=30`],
  [1, "GET /repos/{o}/{r}/contents/", (r) => `/repos/${r}/contents/README.md`],
  [1, "GET /repos/{o}/{r}/branches", (r) => `/repos/${r}/branches`],
  [1, "GET /repos/{o}/{r}/commits", (r) => `/repos/${r}/commits?per_page=10`],
  [1, "GET /repos/{o}/{r}/pulls", (r) => `/repos/${r}/pulls?state=all`],
];

function expand(list, kind) {
  const out = [];
  for (const [w, name, path] of list) for (let i = 0; i < w; i++) out.push({ name, path, kind });
  return out;
}
const MIX = expand(CORE, "core").concat(expand(OPTIONAL, "optional"));

export default function () {
  const repo = pick(fixtureRepos());
  const op = pick(MIX);
  const params = apiParams(op.name, {
    tags: { kind: op.kind },
    responseCallback: op.kind === "optional" ? optionalOk : undefined,
  });
  const res = http.get(API + op.path(repo), params);
  if (op.kind === "core") {
    check(res, { "core 200": (r) => r.status === 200 }, { kind: "core" });
  } else {
    check(res, { "optional 2xx/404": (r) => r.status < 500 }, { kind: "optional" });
  }
}

export const handleSummary = summaryHandler;

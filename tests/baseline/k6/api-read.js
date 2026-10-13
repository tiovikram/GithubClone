// API read mix, equal weights over five semantically equivalent endpoints:
//   GET repo, list issues, get file contents, get user, list the token user's repos.
// forge: /api/v3/..., Forgejo: /api/v1/... (same paths; page size via per_page vs limit).
//   k6 run -e APP=forgejo -e BASE_URL=... -e TOKEN=... -e REPOS=bench/loadfix-1 -e MODE=rate -e RATE=500 api-read.js
import { check } from "k6";
import { API, OWNER, PAGE, http, list, params, perName, pick, scenario, summaryHandler, TREND_STATS } from "./lib.js";

const REPOS = list("REPOS", `${OWNER}/loadfix-1`);
const OPS = [
  ["get_repo", (r) => `/repos/${r}`],
  ["list_issues", (r) => `/repos/${r}/issues?state=open&${PAGE}=30`],
  ["get_contents", (r) => `/repos/${r}/contents/README.md`],
  ["get_user", () => `/users/${OWNER}`],
  ["list_user_repos", () => `/user/repos?${PAGE}=30`],
];

export const options = {
  summaryTrendStats: TREND_STATS,
  discardResponseBodies: false,
  scenarios: { read: scenario() },
  thresholds: perName(OPS.map((o) => o[0])),
};

export default function () {
  const [name, path] = pick(OPS);
  const res = http.get(API + path(pick(REPOS)), params(name));
  check(res, { "200": (r) => r.status === 200 });
}

export const handleSummary = summaryHandler;

// Write mix at a fixed arrival rate: each iteration creates an issue in one of the write
// fixture repos and comments on it (2 requests). Bodies are identical for both apps.
//   k6 run -e APP=forge -e BASE_URL=... -e TOKEN=... -e WREPOS=bench/writefix-1 -e RATE=10 api-write.js
import { check } from "k6";
import { API, OWNER, http, list, params, perName, pick, scenario, summaryHandler, TREND_STATS } from "./lib.js";

const WREPOS = list("WREPOS", `${OWNER}/writefix-1`);

export const options = {
  summaryTrendStats: TREND_STATS,
  scenarios: { write: scenario() },
  thresholds: perName(["create_issue", "create_comment"]),
};

export default function () {
  const repo = pick(WREPOS);
  const issue = http.post(`${API}/repos/${repo}/issues`,
    JSON.stringify({ title: `k6 issue ${__VU}-${__ITER}`, body: "created by the baseline write mix" }),
    params("create_issue"));
  if (!check(issue, { "issue 201": (r) => r.status === 201 })) return;
  const n = issue.json("number");
  const c = http.post(`${API}/repos/${repo}/issues/${n}/comments`,
    JSON.stringify({ body: "k6 comment" }), params("create_comment"));
  check(c, { "comment 201": (r) => r.status === 201 });
}

export const handleSummary = summaryHandler;

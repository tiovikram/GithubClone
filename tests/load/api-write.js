// (c) write mix: create repositories (core), update them (core), create issues and comments
// (optional: 404 until the issues area is merged). Each VU writes into its own repos so the
// per-repo lock is not the bottleneck being measured.
//   k6 run -e BASE_URL=... -e TOKEN=... tests/load/api-write.js
import http from "k6/http";
import { check } from "k6";
import exec from "k6/execution";
import { API, OWNER, apiParams, optionalOk, summaryHandler, TREND_STATS } from "./lib.js";

const VUS = Number(__ENV.VUS || 5);
const RATE = Number(__ENV.WRITE_RATE || 50);
const DURATION = __ENV.DURATION || "60s";
const RUN = __ENV.RUN_ID || String(Date.now());

export const options = {
  summaryTrendStats: TREND_STATS,
  scenarios: {
    // Fixed arrival rate (not a closed loop): writes are measured at a steady RATE ops/s
    // instead of flooding the database with tens of thousands of repositories.
    write: {
      executor: "constant-arrival-rate",
      rate: RATE,
      timeUnit: "1s",
      duration: DURATION,
      preAllocatedVUs: VUS,
      maxVUs: VUS * 4,
    },
  },
  thresholds: {
    "http_req_failed{kind:core}": ["rate<0.01"],
    "http_req_duration{name:POST /user/repos}": ["p(95)<800"],
    "http_req_duration{name:PATCH /repos/{o}/{r}}": ["p(95)<500"],
    "http_req_duration{kind:optional}": ["p(95)<1000"],
    checks: ["rate>0.99"],
  },
};

const mine = {}; // per-VU list of repos created in this run

export default function () {
  const vu = exec.vu.idInTest;
  const it = exec.vu.iterationInScenario;
  mine[vu] = mine[vu] || [];
  const roll = Math.random();

  if (mine[vu].length === 0 || roll < 0.2) {
    const name = `load-${RUN}-${vu}-${it}`;
    const res = http.post(
      `${API}/user/repos`,
      JSON.stringify({ name, description: "k6 write mix", auto_init: false }),
      apiParams("POST /user/repos", { tags: { kind: "core" } }),
    );
    if (check(res, { "create repo 201": (r) => r.status === 201 }, { kind: "core" })) mine[vu].push(name);
    return;
  }
  const repo = mine[vu][Math.floor(Math.random() * mine[vu].length)];
  if (roll < 0.4) {
    const res = http.patch(
      `${API}/repos/${OWNER}/${repo}`,
      JSON.stringify({ description: `updated ${it}`, homepage: "https://example.com" }),
      apiParams("PATCH /repos/{o}/{r}", { tags: { kind: "core" } }),
    );
    check(res, { "patch repo 200": (r) => r.status === 200 }, { kind: "core" });
    return;
  }
  // Issues + comments: tolerated 404 until the issues area is merged.
  const opt = { tags: { kind: "optional" }, responseCallback: optionalOk };
  const issue = http.post(
    `${API}/repos/${OWNER}/${repo}/issues`,
    JSON.stringify({ title: `load issue ${it}`, body: "created by k6" }),
    apiParams("POST /repos/{o}/{r}/issues", opt),
  );
  check(issue, { "create issue 201|404": (r) => r.status === 201 || r.status === 404 }, { kind: "optional" });
  if (issue.status === 201) {
    const n = issue.json("number");
    const c = http.post(
      `${API}/repos/${OWNER}/${repo}/issues/${n}/comments`,
      JSON.stringify({ body: "k6 comment" }),
      apiParams("POST /repos/{o}/{r}/issues/{n}/comments", opt),
    );
    check(c, { "comment 201|404": (r) => r.status === 201 || r.status === 404 }, { kind: "optional" });
  }
}

export const handleSummary = summaryHandler;

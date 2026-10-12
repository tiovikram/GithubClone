// Shared helpers for forge k6 scripts.
import http from "k6/http";
import encoding from "k6/encoding";

export const BASE = (__ENV.BASE_URL || "http://localhost:8080").replace(/\/$/, "");
export const API = `${BASE}/api/v3`;
export const TOKEN = __ENV.TOKEN || "";
export const OWNER = __ENV.OWNER || "admin";
// Chaos runs lower this so requests stuck on a killed pod fail fast instead of stalling VUs.
export const HTTP_TIMEOUT = __ENV.HTTP_TIMEOUT || "60s";
export const TREND_STATS =["avg", "min", "med", "p(90)", "p(95)", "p(99)", "max"];

export function apiParams(name, extra = {}) {
  return {
    headers: {
      Authorization: `token ${TOKEN}`,
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
      "Content-Type": "application/json",
    },
    tags: { name, ...(extra.tags || {}) },
    timeout: HTTP_TIMEOUT,
    ...(extra.responseCallback ? { responseCallback: extra.responseCallback } : {}),
  };
}

// Endpoints that other areas are still implementing: a 404/405/501 is "not there yet", not an
// error. They are tagged optional so thresholds can focus on what exists.
export const optionalOk = http.expectedStatuses(200, 201, 204, 404, 405, 422, 501);

export function basicAuth() {
  return "Basic " + encoding.b64encode(`x-access-token:${TOKEN}`);
}

// Comma-separated list of repos prepared by run.sh (owner/name).
export function fixtureRepos() {
  return (__ENV.REPOS || `${OWNER}/loadfix`).split(",").filter(Boolean);
}

export function pick(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

// Writes the end-of-test summary as JSON for run.sh plus the default text summary on stdout.
export function summaryHandler(data) {
  const out = {};
  if (__ENV.SUMMARY_JSON) out[__ENV.SUMMARY_JSON] = JSON.stringify(data, null, 1);
  out.stdout = textSummary(data);
  return out;
}

function fmt(v) {
  return v === undefined ? "-" : Number(v).toFixed(1);
}

function textSummary(data) {
  const m = data.metrics;
  const lines = [];
  for (const [k, v] of Object.entries(m).sort()) {
    if (!v.values) continue;
    const vals = v.values;
    if (v.type === "trend") {
      lines.push(`${k}: avg=${fmt(vals.avg)} p50=${fmt(vals.med)} p95=${fmt(vals["p(95)"])} p99=${fmt(vals["p(99)"])} max=${fmt(vals.max)}`);
    } else if (v.type === "rate") {
      lines.push(`${k}: ${(vals.rate * 100).toFixed(2)}%`);
    } else if (v.type === "counter") {
      lines.push(`${k}: ${vals.count} (${fmt(vals.rate)}/s)`);
    }
    if (v.thresholds) {
      for (const [t, r] of Object.entries(v.thresholds)) lines.push(`  threshold ${t}: ${r.ok ? "PASS" : "FAIL"}`);
    }
  }
  return lines.join("\n") + "\n";
}

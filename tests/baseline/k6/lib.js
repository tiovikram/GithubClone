// Shared helpers for the forge-vs-Forgejo baseline. The only per-app differences are the API
// prefix (forge /api/v3, Forgejo /api/v1) and the page-size query parameter (per_page / limit);
// paths, request bodies and auth header are identical.
import http from "k6/http";

export const APP = __ENV.APP || "forge"; // forge | forgejo
export const BASE = (__ENV.BASE_URL || "http://localhost:8088").replace(/\/$/, "");
export const API = BASE + (__ENV.API_PREFIX || (APP === "forgejo" ? "/api/v1" : "/api/v3"));
export const PAGE = APP === "forgejo" ? "limit" : "per_page";
export const TOKEN = __ENV.TOKEN || "";
export const OWNER = __ENV.OWNER || "bench";
export const HTTP_TIMEOUT = __ENV.HTTP_TIMEOUT || "30s";
export const DURATION = __ENV.DURATION || "60s";
export const TREND_STATS = ["avg", "min", "med", "p(90)", "p(95)", "p(99)", "max"];

export function params(name) {
  return {
    headers: {
      Authorization: `token ${TOKEN}`,
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    tags: { name },
    timeout: HTTP_TIMEOUT,
  };
}

export function list(env, dflt) {
  return (__ENV[env] || dflt).split(",").filter(Boolean);
}

export function pick(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

// MODE=rate: open model, constant arrival rate RATE/s (dropped_iterations = server could not
// keep up with VUS_MAX in-flight requests). MODE=vus: closed loop with VUS virtual users
// (saturation: throughput is whatever the server sustains).
export function scenario() {
  if ((__ENV.MODE || "rate") === "vus") {
    return { executor: "constant-vus", vus: Number(__ENV.VUS || 64), duration: DURATION };
  }
  const rate = Number(__ENV.RATE || 100);
  return {
    executor: "constant-arrival-rate",
    rate,
    timeUnit: "1s",
    duration: DURATION,
    preAllocatedVUs: Math.max(20, Math.ceil(rate / 10)),
    // Cap on in-flight requests. Must stay below ~500: the k3d server load balancer (nginx,
    // worker_connections 1024 = ~512 proxied connections) resets everything above that with an
    // EOF, which would measure the LB instead of the app. Overload then shows up as
    // dropped_iterations (open model) rather than as LB resets.
    maxVUs: Number(__ENV.VUS_MAX || 400),
  };
}

// Per-name sub-metrics only appear in the summary when a threshold references them, so every
// named request gets a never-failing threshold.
export function perName(names) {
  const t = {};
  for (const n of names) {
    t[`http_req_duration{name:${n}}`] = ["p(95)>=0"];
    t[`http_req_failed{name:${n}}`] = ["rate>=0"];
  }
  return t;
}

export function summaryHandler(data) {
  const out = { stdout: textSummary(data) };
  if (__ENV.SUMMARY_JSON) out[__ENV.SUMMARY_JSON] = JSON.stringify(data, null, 1);
  return out;
}

function fmt(v) {
  return v === undefined ? "-" : Number(v).toFixed(1);
}

function textSummary(data) {
  const lines = [];
  for (const [k, v] of Object.entries(data.metrics).sort()) {
    const x = v.values || {};
    if (v.type === "trend") lines.push(`${k}: avg=${fmt(x.avg)} p50=${fmt(x.med)} p95=${fmt(x["p(95)"])} p99=${fmt(x["p(99)"])} max=${fmt(x.max)}`);
    else if (v.type === "rate") lines.push(`${k}: ${(x.rate * 100).toFixed(2)}%`);
    else if (v.type === "counter") lines.push(`${k}: ${x.count} (${fmt(x.rate)}/s)`);
  }
  return lines.join("\n") + "\n";
}

export { http };

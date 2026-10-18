// Shared bits for the actions area: status icons, durations, polling, dropdown menus.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { matchPath, useLocation } from "react-router-dom";
import type { Conclusion, Status } from "./api";

/** Normalised state of a run / job / step / check / commit status. */
export type Kind = "success" | "failure" | "cancelled" | "skipped" | "neutral" | "in_progress" | "queued" | "waiting" | "pending";

export function kindOf(status: Status | undefined, conclusion?: Conclusion): Kind {
  if (status === "completed" || (!status && conclusion)) {
    switch (conclusion) {
      case "success":
        return "success";
      case "failure":
      case "timed_out":
      case "startup_failure":
      case "error":
        return "failure";
      case "cancelled":
        return "cancelled";
      case "skipped":
        return "skipped";
      case "action_required":
        return "waiting";
      default:
        return "neutral";
    }
  }
  if (status === "in_progress") return "in_progress";
  if (status === "waiting" || status === "action_required") return "waiting";
  if (status === "pending") return "pending";
  return "queued";
}

/** Commit status / combined status state → kind. */
export function statusKind(state: string): Kind {
  if (state === "success") return "success";
  if (state === "failure" || state === "error") return "failure";
  return "pending";
}

const LABELS: Record<Kind, string> = {
  success: "Success",
  failure: "Failure",
  cancelled: "Cancelled",
  skipped: "Skipped",
  neutral: "Neutral",
  in_progress: "In progress",
  queued: "Queued",
  waiting: "Waiting",
  pending: "Pending",
};

export const kindLabel = (k: Kind) => LABELS[k];

/** 16px status icon in GitHub's style (check, x, spinner, dot, slash). */
export function StatusIcon({ kind, size = 16, title }: { kind: Kind; size?: number; title?: string }) {
  const t = title ?? LABELS[kind];
  const common = { width: size, height: size, viewBox: "0 0 16 16", role: "img", "aria-label": t, className: `act-icon act-${kind}` } as const;
  switch (kind) {
    case "success":
      return (
        <svg {...common}><title>{t}</title><circle cx="8" cy="8" r="7" fill="currentColor" /><path d="m4.8 8.2 2.2 2.2 4.2-4.6" fill="none" stroke="var(--bg)" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" /></svg>
      );
    case "failure":
      return (
        <svg {...common}><title>{t}</title><circle cx="8" cy="8" r="7" fill="currentColor" /><path d="m5.5 5.5 5 5m0-5-5 5" stroke="var(--bg)" strokeWidth="1.6" strokeLinecap="round" /></svg>
      );
    case "cancelled":
    case "skipped":
      return (
        <svg {...common}><title>{t}</title><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" strokeWidth="1.5" /><path d="m4 12 8-8" stroke="currentColor" strokeWidth="1.5" /></svg>
      );
    case "in_progress":
      return (
        <svg {...common}><title>{t}</title><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" strokeOpacity=".25" strokeWidth="2" /><path className="act-spin" d="M8 2a6 6 0 0 1 6 6" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" /></svg>
      );
    case "waiting":
      return (
        <svg {...common}><title>{t}</title><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" strokeWidth="1.5" /><path d="M8 4.5V8l2.2 1.6" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" /></svg>
      );
    case "neutral":
      return (
        <svg {...common}><title>{t}</title><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" strokeWidth="1.5" /><path d="M5 8h6" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" /></svg>
      );
    default:
      return (
        <svg {...common}><title>{t}</title><circle cx="8" cy="8" r="3.5" fill="currentColor" /></svg>
      );
  }
}

/** "1m 5s" between two timestamps (or until now while running). */
export function duration(from?: string | null, to?: string | null): string {
  if (!from) return "";
  const a = Date.parse(from);
  const b = to ? Date.parse(to) : Date.now();
  if (Number.isNaN(a) || Number.isNaN(b)) return "";
  let s = Math.max(0, Math.round((b - a) / 1000));
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  s -= m * 60;
  return h ? `${h}h ${m}m ${s}s` : m ? `${m}m ${s}s` : `${s}s`;
}

/**
 * Which of `patterns` (relative to the absolute `root`, e.g. "/environments/:id/edit") matches
 * the current location. Settings sections use this instead of nested <Routes> so they do not
 * depend on how the hosting settings shell declares its route.
 */
export function useSubRoute(root: string, patterns: string[]): { pattern: string; params: Record<string, string | undefined> } | null {
  const { pathname } = useLocation();
  const p = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  for (const pattern of patterns) {
    const m = matchPath({ path: root + pattern, end: true }, p);
    if (m) return { pattern, params: m.params };
  }
  return null;
}

/** Calls `fn` every `ms` while `active`. */
export function usePoll(fn: () => void, ms: number, active: boolean) {
  const ref = useRef(fn);
  ref.current = fn;
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => ref.current(), ms);
    return () => clearInterval(t);
  }, [ms, active]);
}

/** A button that opens a small menu; closes on outside click / Escape. */
export function Dropdown({ label, children, className = "btn btn-sm", align = "right", testid, ariaLabel }: {
  label: ReactNode;
  children: (close: () => void) => ReactNode;
  className?: string;
  align?: "left" | "right";
  testid?: string;
  ariaLabel?: string;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof KeyboardEvent ? e.key === "Escape" : !ref.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", close);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", close);
    };
  }, [open]);
  return (
    <div className="menu-wrap act-dropdown" ref={ref}>
      <button type="button" className={className} aria-haspopup="menu" aria-expanded={open} aria-label={ariaLabel}
        onClick={() => setOpen(!open)} data-testid={testid}>
        {label}
      </button>
      {open && <div className={`menu act-menu act-menu-${align}`} role="menu">{children(() => setOpen(false))}</div>}
    </div>
  );
}

/** Byte size like GitHub's artifact list ("112 Bytes", "1.2 MB"). */
export function bytes(n: number): string {
  if (n < 1024) return `${n} Bytes`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export const EVENTS = [
  "push", "pull_request", "pull_request_target", "workflow_dispatch", "schedule", "repository_dispatch",
  "workflow_run", "workflow_call", "release", "issues", "issue_comment", "create", "delete", "merge_group", "dynamic",
];

export const STATUSES = [
  "success", "failure", "cancelled", "skipped", "in_progress", "queued", "waiting", "action_required", "neutral", "timed_out", "completed", "requested", "pending",
];

/** Human event name used in run rows ("Manually run by", "pushed by"). */
export function eventVerb(event: string): string {
  switch (event) {
    case "push":
      return "Commit pushed by";
    case "workflow_dispatch":
      return "Manually run by";
    case "schedule":
      return "Scheduled";
    case "pull_request":
    case "pull_request_target":
      return "Pull request opened by";
    case "repository_dispatch":
      return "Repository dispatch by";
    default:
      return `${event} triggered by`;
  }
}

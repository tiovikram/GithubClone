// Small building blocks shared by the site admin pages.
import { useCallback, useState, type ReactNode } from "react";
import { ErrorBox, Flash } from "../../components/common";

/** Runs an async action with busy / error / success state. */
export function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const [done, setDone] = useState<string>();
  const run = useCallback(async (fn: () => Promise<unknown>, success?: string) => {
    setBusy(true);
    setError(undefined);
    setDone(undefined);
    try {
      await fn();
      if (success) setDone(success);
      return true;
    } catch (e) {
      setError(e);
      return false;
    } finally {
      setBusy(false);
    }
  }, []);
  const status = (
    <>
      {error !== undefined && <ErrorBox error={error} />}
      {done && <Flash kind="success">{done}</Flash>}
    </>
  );
  return { busy, error, done, run, status, setDone, setError };
}

export function PageHead({ title, children, desc }: { title: string; children?: ReactNode; desc?: ReactNode }) {
  return (
    <div className="sa-head">
      <div className="sa-head-row">
        <h2 className="h3">{title}</h2>
        {children && <div className="sa-head-actions">{children}</div>}
      </div>
      {desc && <p className="muted small">{desc}</p>}
    </div>
  );
}

const tone: Record<string, string> = {
  on: "sa-badge-danger", enabled: "sa-badge-success", success: "sa-badge-success", OK: "sa-badge-success", valid: "sa-badge-success",
  scheduled: "sa-badge-warn", testing: "sa-badge-warn", in_progress: "sa-badge-warn", paused: "sa-badge-warn", created: "sa-badge-warn",
  failed: "sa-badge-danger", CRITICAL: "sa-badge-danger", suspended: "sa-badge-danger", cancelled: "", disabled: "", off: "",
  "site admin": "sa-badge-accent", owner: "sa-badge-accent", admin: "sa-badge-accent",
};

export function Pill({ children, kind }: { children: string; kind?: string }) {
  return <span className={`badge ${tone[kind ?? children] ?? ""}`}>{children}</span>;
}

/** A labelled stat number. */
export function Stat({ label, value, testid }: { label: string; value: number | string | undefined; testid?: string }) {
  return (
    <div className="sa-stat" data-testid={testid}>
      <div className="sa-stat-value">{value ?? "—"}</div>
      <div className="sa-stat-label muted">{label}</div>
    </div>
  );
}

/** `<input type="datetime-local">` value (local time) ↔ ISO 8601 (UTC). */
export function toLocalInput(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function fromLocalInput(v: string): string | null {
  if (!v) return null;
  const d = new Date(v);
  return isNaN(d.getTime()) ? null : d.toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** Download `text` as a file. */
export function download(name: string, text: string, type: string) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Simple page-number pager for listings with a total count. */
export function Pager({ page, total, per, onPage }: { page: number; total: number; per: number; onPage: (p: number) => void }) {
  const last = Math.max(1, Math.ceil(total / per));
  if (last <= 1) return null;
  return (
    <nav className="pagination" aria-label="Pagination">
      <button type="button" className="btn btn-sm" disabled={page <= 1} onClick={() => onPage(page - 1)}>‹ Previous</button>
      <span className="muted">Page {page} of {last}</span>
      <button type="button" className="btn btn-sm" disabled={page >= last} onClick={() => onPage(page + 1)}>Next ›</button>
    </nav>
  );
}

// Small shared pieces of the security area UI.
import { useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { errorMessage } from "../../api";
import { severityLabel, type Severity } from "./api";

export function SeverityLabel({ severity }: { severity: Severity | null | undefined }) {
  const s = (severity || "unknown").toLowerCase();
  return <span className={`sec-sev sec-sev-${s}`} data-testid="severity">{severityLabel(s)}</span>;
}

/** Open / Closed / Dismissed / Fixed state pill. */
export function AlertState({ state }: { state: string }) {
  const label = state === "auto_dismissed" ? "Auto-dismissed" : state[0].toUpperCase() + state.slice(1).replace(/_/g, " ");
  const kind = state === "open" || state === "draft" || state === "triage" || state === "pending" ? "open"
    : state === "fixed" || state === "published" || state === "approved" ? "done" : "closed";
  return <span className={`sec-state sec-state-${kind}`} data-testid="alert-state">{label}</span>;
}

export function ShieldIcon({ size = 16, className = "icon" }: { size?: number; className?: string }) {
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path d="M7.467.133a1.748 1.748 0 0 1 1.066 0l5.25 1.68A1.75 1.75 0 0 1 15 3.48V7c0 1.566-.32 3.182-1.303 4.682-.983 1.498-2.585 2.813-5.032 3.855a1.697 1.697 0 0 1-1.33 0c-2.447-1.042-4.049-2.357-5.032-3.855C1.32 10.182 1 8.566 1 7V3.48a1.755 1.755 0 0 1 1.217-1.667Zm.61 1.429a.25.25 0 0 0-.153 0l-5.25 1.68a.25.25 0 0 0-.174.238V7c0 1.358.275 2.666 1.057 3.86.784 1.194 2.121 2.34 4.366 3.297a.196.196 0 0 0 .154 0c2.245-.956 3.582-2.104 4.366-3.298C13.225 9.666 13.5 8.36 13.5 7V3.48a.251.251 0 0 0-.174-.237l-5.25-1.68ZM8.75 4.75v3a.75.75 0 0 1-1.5 0v-3a.75.75 0 0 1 1.5 0ZM9 10.5a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z" />
    </svg>
  );
}

/** A heading row for a security page. */
export function PageHead({ title, children, sub }: { title: ReactNode; children?: ReactNode; sub?: ReactNode }) {
  return (
    <div className="sec-head">
      <div className="sec-head-main">
        <h2 className="sec-title">{title}</h2>
        {sub && <p className="muted sec-sub">{sub}</p>}
      </div>
      {children && <div className="sec-head-actions">{children}</div>}
    </div>
  );
}

/** State filter tabs (Open 3 / Closed 1). */
export function StateTabs({ value, onChange, tabs }: {
  value: string;
  onChange: (v: string) => void;
  tabs: { key: string; label: string; count?: number }[];
}) {
  return (
    <div className="list-tabs sec-tabs" role="tablist">
      {tabs.map((t) => (
        <button key={t.key} type="button" role="tab" aria-selected={value === t.key}
          className={value === t.key ? "selected" : ""} onClick={() => onChange(t.key)} data-testid={`state-tab-${t.key}`}>
          {t.label}{t.count !== undefined && <span className="counter">{t.count}</span>}
        </button>
      ))}
    </div>
  );
}

/** A labelled select used in list filter bars. */
export function FilterSelect({ label, value, onChange, options, id }: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: [string, string][];
  id: string;
}) {
  return (
    <label className="sec-filter" htmlFor={id}>
      <span className="sr-only">{label}</span>
      <select id={id} className="input input-sm input-auto" value={value} onChange={(e) => onChange(e.target.value)} aria-label={label}>
        <option value="">{label}: all</option>
        {options.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
      </select>
    </label>
  );
}

/** Run an async action with busy / error state. */
export function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    try {
      await fn();
      return true;
    } catch (e) {
      setError(e);
      return false;
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, setError, run };
}

export function InlineError({ error }: { error: unknown }) {
  if (!error) return null;
  return <div className="flash flash-error" role="alert">{errorMessage(error)}</div>;
}

/** "Feature disabled" blank state with a link to the settings page for admins. */
export function FeatureOff({ title, children, settings, isAdmin }: { title: string; children: ReactNode; settings: string; isAdmin: boolean }) {
  return (
    <div className="blankslate sec-off" data-testid="feature-off">
      <div className="blankslate-icon"><ShieldIcon size={24} /></div>
      <h3>{title}</h3>
      <div className="muted">{children}</div>
      {isAdmin && (
        <p><Link className="btn btn-sm" to={settings}>Go to Advanced Security settings</Link></p>
      )}
    </div>
  );
}

/** A setting row with a title, description and an Enable/Disable button. */
export function ToggleRow({ title, description, on, onToggle, busy, disabled, testid, children }: {
  title: string;
  description: ReactNode;
  on: boolean;
  onToggle: (next: boolean) => void;
  busy?: boolean;
  disabled?: boolean;
  testid: string;
  children?: ReactNode;
}) {
  return (
    <div className="sec-toggle-row" data-testid={`toggle-${testid}`}>
      <div className="sec-toggle-text">
        <h3 className="sec-toggle-title">{title} <span className={`badge${on ? " badge-added" : ""}`}>{on ? "Enabled" : "Disabled"}</span></h3>
        <div className="muted small">{description}</div>
        {children}
      </div>
      <button type="button" className={`btn btn-sm${on ? " btn-danger-outline" : ""}`} disabled={busy || disabled}
        onClick={() => onToggle(!on)} data-testid={`toggle-${testid}-btn`}>
        {on ? "Disable" : "Enable"}
      </button>
    </div>
  );
}

export function download(name: string, text: string, type = "application/json") {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

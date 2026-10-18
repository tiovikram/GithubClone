// Small shared building blocks for the repoadmin pages.
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { errorMessage, type User } from "../../api";
import { Avatar } from "../../components/common";
import { ra } from "./api";

/** Page heading of a settings section: title, optional description and right-side actions. */
export function SectionHead({ title, children, actions, level = 2 }: { title: ReactNode; children?: ReactNode; actions?: ReactNode; level?: 2 | 3 }) {
  const H = level === 2 ? "h2" : "h3";
  return (
    <div className="ra-head">
      <div className="ra-head-row">
        <H className={level === 2 ? "h3 ra-title" : "h4 ra-title"}>{title}</H>
        {actions && <div className="ra-head-actions">{actions}</div>}
      </div>
      {children && <div className="muted ra-desc">{children}</div>}
    </div>
  );
}

/** A labelled checkbox with an optional description, GitHub settings style. */
export function Check({ id, label, desc, checked, onChange, disabled, children }: {
  id: string;
  label: ReactNode;
  desc?: ReactNode;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className="ra-check">
      <label className="check-row" htmlFor={id}>
        <input id={id} type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
        <span>
          <strong>{label}</strong>
          {desc && <span className="ra-check-desc muted small">{desc}</span>}
        </span>
      </label>
      {children && checked && <div className="ra-check-children">{children}</div>}
    </div>
  );
}

/** Inline status line after an action: "Saved." or the error. */
export function Status({ state }: { state: { ok?: string; err?: unknown } }) {
  if (state.err !== undefined) return <div className="flash flash-error" role="alert">{errorMessage(state.err)}</div>;
  if (state.ok) return <div className="flash flash-success" role="status">{state.ok}</div>;
  return null;
}

/** Run an async action with busy/ok/error state. */
export function useAction() {
  const [busy, setBusy] = useState(false);
  const [state, setState] = useState<{ ok?: string; err?: unknown }>({});
  const run = async <T,>(fn: () => Promise<T>, ok?: string): Promise<T | undefined> => {
    setBusy(true);
    setState({});
    try {
      const v = await fn();
      setState({ ok });
      return v;
    } catch (e) {
      setState({ err: e });
      return undefined;
    } finally {
      setBusy(false);
    }
  };
  return { busy, state, run, setState };
}

/** A row of the Danger Zone with a button that expands a confirmation form. */
export function DangerRow({ title, desc, button, children, testid }: {
  title: string;
  desc: ReactNode;
  button: string;
  children: (close: () => void) => ReactNode;
  testid?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="ra-danger-row" data-testid={testid}>
      <div className="ra-danger-main">
        <div className="ra-danger-text">
          <strong>{title}</strong>
          <div className="small muted">{desc}</div>
        </div>
        {!open && <button type="button" className="btn btn-danger btn-sm" onClick={() => setOpen(true)}>{button}</button>}
      </div>
      {open && <div className="ra-danger-form">{children(() => setOpen(false))}</div>}
    </div>
  );
}

/** Text input with user suggestions from the user search API. */
export function UserInput({ id, value, onChange, placeholder = "Search by username", label }: {
  id: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  label?: string;
}) {
  const [opts, setOpts] = useState<User[]>([]);
  const list = useId();
  const t = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => {
    clearTimeout(t.current);
    const q = value.trim();
    if (q.length < 2) {
      setOpts([]);
      return;
    }
    t.current = setTimeout(() => {
      ra.searchUsers(`${q} in:login`).then((r) => setOpts(r.items || []), () => setOpts([]));
    }, 250);
    return () => clearTimeout(t.current);
  }, [value]);
  return (
    <>
      <input id={id} className="input" value={value} placeholder={placeholder} aria-label={label} autoComplete="off" list={list}
        onChange={(e) => onChange(e.target.value)} />
      <datalist id={list}>
        {opts.map((u) => <option key={u.login} value={u.login} />)}
      </datalist>
    </>
  );
}

export function Who({ user, size = 20 }: { user: Pick<User, "login" | "avatar_url"> | null | undefined; size?: number }) {
  return (
    <span className="ra-who">
      <Avatar user={user} size={size} />
      <span>{user?.login || "ghost"}</span>
    </span>
  );
}

/** Comma/newline separated list editor (for patterns, contexts, …). */
export function ListInput({ id, value, onChange, placeholder, rows = 3 }: {
  id: string;
  value: string[];
  onChange: (v: string[]) => void;
  placeholder?: string;
  rows?: number;
}) {
  const [text, setText] = useState(value.join("\n"));
  useEffect(() => {
    if (split(text).join("\n") !== value.join("\n")) setText(value.join("\n"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value.join("\n")]);
  return (
    <textarea id={id} className="input textarea mono ra-list-input" rows={rows} value={text} placeholder={placeholder}
      onChange={(e) => {
        setText(e.target.value);
        onChange(split(e.target.value));
      }} />
  );
}

export const split = (s: string) => s.split(/[\n,]/).map((x) => x.trim()).filter(Boolean);

/** Label/value pill. */
export function Pill({ kind = "", children }: { kind?: "" | "ok" | "warn" | "bad" | "info"; children: ReactNode }) {
  return <span className={`ra-pill${kind ? ` ra-pill-${kind}` : ""}`}>{children}</span>;
}

export function JsonBlock({ value }: { value: unknown }) {
  let text: string;
  if (typeof value === "string") {
    try {
      text = JSON.stringify(JSON.parse(value), null, 2);
    } catch {
      text = value;
    }
  } else text = JSON.stringify(value, null, 2);
  return <pre className="ra-pre mono">{text}</pre>;
}

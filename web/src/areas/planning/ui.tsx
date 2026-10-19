// Small UI building blocks shared by the planning pages: popover menus, select menus, dialogs,
// progress bars, the "…" comment menu and reaction bars.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { errorMessage, type User } from "../../api";
import { useAuth } from "../../auth";
import { Avatar, Spinner } from "../../components/common";
import { papi, type Reaction, type ReactionContent, type Reactions } from "./api";

/** Close a popover on outside click / Escape. */
export function useDismiss(open: boolean, close: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, close]);
  return ref;
}

/** A button that opens a popover with arbitrary content. */
export function Popover({ label, button, children, className = "btn btn-sm", align = "right", testid, onOpen }: {
  onOpen?: () => void;
  label: string;
  button: ReactNode;
  children: (close: () => void) => ReactNode;
  className?: string;
  align?: "left" | "right";
  testid?: string;
}) {
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);
  const ref = useDismiss(open, close);
  return (
    <div className="pl-popover-wrap" ref={ref}>
      <button type="button" className={className} aria-label={label} aria-expanded={open} aria-haspopup="true"
        onClick={() => { if (!open) onOpen?.(); setOpen(!open); }} data-testid={testid}>
        {button}
      </button>
      {open && <div className={`pl-popover pl-popover-${align}`} role="dialog" aria-label={label}>{children(close)}</div>}
    </div>
  );
}

export interface SelectOption {
  key: string;
  label: ReactNode;
  /** Text matched by the filter box. */
  text: string;
  description?: ReactNode;
}

/**
 * GitHub-style select menu: a filterable checklist. Single mode applies on click; multi mode
 * applies when the menu closes (or with the Apply button).
 */
export function SelectMenu({ title, options, selected, multi = false, onApply, filterPlaceholder = "Filter", footer, loading, emptyText = "No results" }: {
  title: string;
  options: SelectOption[];
  selected: string[];
  multi?: boolean;
  onApply: (keys: string[]) => void | Promise<void>;
  filterPlaceholder?: string;
  footer?: ReactNode;
  loading?: boolean;
  emptyText?: string;
}) {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState<Set<string>>(new Set(selected));
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const shown = options.filter((o) => o.text.toLowerCase().includes(q.trim().toLowerCase()));
  const apply = async (keys: string[]) => {
    setBusy(true);
    setErr("");
    try {
      await onApply(keys);
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="pl-select" role="group" aria-label={title}>
      <div className="pl-select-title">{title}</div>
      <div className="pl-select-filter">
        <input className="input input-sm" autoFocus placeholder={filterPlaceholder} aria-label={filterPlaceholder}
          value={q} onChange={(e) => setQ(e.target.value)} />
      </div>
      <div className="pl-select-list">
        {loading ? <Spinner /> : shown.length === 0 ? <div className="pl-select-empty muted small">{emptyText}</div> :
          shown.map((o) => {
            const on = sel.has(o.key);
            return (
              <button type="button" key={o.key} className={"pl-select-item" + (on ? " selected" : "")} role="menuitemcheckbox"
                aria-checked={on} data-key={o.key} disabled={busy}
                onClick={() => {
                  if (multi) {
                    const s = new Set(sel);
                    if (on) s.delete(o.key); else s.add(o.key);
                    setSel(s);
                  } else {
                    apply(on ? [] : [o.key]);
                  }
                }}>
                <span className="pl-check" aria-hidden="true">{on ? "✓" : ""}</span>
                <span className="pl-select-label">
                  {o.label}
                  {o.description && <span className="muted small pl-select-desc">{o.description}</span>}
                </span>
              </button>
            );
          })}
      </div>
      {err && <p className="c-danger small pl-select-err" role="alert">{err}</p>}
      {multi && (
        <div className="pl-select-foot">
          <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => apply([...sel])}>
            {busy ? "Applying…" : "Apply"}
          </button>
        </div>
      )}
      {footer && <div className="pl-select-foot">{footer}</div>}
    </div>
  );
}

export function userOption(u: User): SelectOption {
  return {
    key: u.login,
    text: `${u.login} ${u.name || ""}`,
    label: <span className="pl-user-opt"><Avatar user={u} size={20} /> <strong>{u.login}</strong> {u.name && <span className="muted">{u.name}</span>}</span>,
  };
}

/** Modal dialog. */
export function Dialog({ title, onClose, children, wide }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="pl-dialog-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={"pl-dialog box" + (wide ? " pl-dialog-wide" : "")} role="dialog" aria-modal="true" aria-label={title}>
        <div className="pl-dialog-head">
          <h2 className="h4">{title}</h2>
          <button type="button" className="btn-link btn-icon" aria-label="Close" onClick={onClose}>✕</button>
        </div>
        <div className="pl-dialog-body">{children}</div>
      </div>
    </div>
  );
}

export function Progress({ done, total, label }: { done: number; total: number; label?: string }) {
  const pct = total ? Math.round((done / total) * 100) : 0;
  return (
    <span className="pl-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}
      aria-label={label || `${pct}% complete`}>
      <span className="pl-progress-bar" style={{ width: `${pct}%` }} />
    </span>
  );
}

/** Kebab "…" menu with items. */
export function KebabMenu({ label, items }: { label: string; items: { label: string; onClick: () => void; danger?: boolean; hidden?: boolean }[] }) {
  const visible = items.filter((i) => !i.hidden);
  if (!visible.length) return null;
  return (
    <Popover label={label} button={<span aria-hidden="true">···</span>} className="btn-link btn-icon pl-kebab">
      {(close) => (
        <div className="pl-menu" role="menu">
          {visible.map((i) => (
            <button key={i.label} type="button" role="menuitem" className={"menu-item" + (i.danger ? " c-danger" : "")}
              onClick={() => { close(); i.onClick(); }}>
              {i.label}
            </button>
          ))}
        </div>
      )}
    </Popover>
  );
}

// ---------- reactions ----------

export const REACTIONS: { content: ReactionContent; emoji: string; label: string }[] = [
  { content: "+1", emoji: "👍", label: "+1" },
  { content: "-1", emoji: "👎", label: "-1" },
  { content: "laugh", emoji: "😄", label: "laugh" },
  { content: "hooray", emoji: "🎉", label: "hooray" },
  { content: "confused", emoji: "😕", label: "confused" },
  { content: "heart", emoji: "❤️", label: "heart" },
  { content: "rocket", emoji: "🚀", label: "rocket" },
  { content: "eyes", emoji: "👀", label: "eyes" },
];

/**
 * Reaction summary + picker for a subject (`issues/3`, `issues/comments/9`, `pulls/comments/4`).
 * Loads the individual reactions when there are any, to know which are the viewer's.
 */
export function ReactionBar({ owner, name, subject, summary, readOnly }: {
  owner: string;
  name: string;
  subject: string;
  summary?: Reactions;
  readOnly?: boolean;
}) {
  const { user } = useAuth();
  const [list, setList] = useState<Reaction[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  useEffect(() => {
    let live = true;
    if (summary && summary.total_count === 0) {
      setList([]);
      return;
    }
    papi.reactions(owner, name, subject).then((l) => live && setList(l)).catch(() => live && setList([]));
    return () => { live = false; };
  }, [owner, name, subject, summary?.total_count]);
  const toggle = async (content: ReactionContent) => {
    if (!user || !list) return;
    setBusy(true);
    setErr("");
    try {
      const mine = list.find((r) => r.content === content && r.user?.login === user.login);
      if (mine) {
        await papi.unreact(owner, name, subject, mine.id);
        setList(list.filter((r) => r.id !== mine.id));
      } else {
        const r = await papi.react(owner, name, subject, content);
        setList(list.some((x) => x.id === r.id) ? list : [...list, r]);
      }
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const counts = REACTIONS.map((r) => ({
    ...r,
    users: (list || []).filter((x) => x.content === r.content).map((x) => x.user?.login || "ghost"),
  })).filter((r) => r.users.length > 0);
  if (!list) return null;
  if (!counts.length && (!user || readOnly)) return null;
  return (
    <div className="pl-reactions" data-testid="reactions">
      {counts.map((r) => {
        const mine = !!user && r.users.includes(user.login);
        return (
          <button key={r.content} type="button" className={"pl-reaction" + (mine ? " mine" : "")} disabled={busy || !user || readOnly}
            aria-pressed={mine} aria-label={`${r.label} reaction (${r.users.length})`} title={`${r.users.join(", ")} reacted with ${r.label}`}
            data-reaction={r.content} onClick={() => toggle(r.content)}>
            <span aria-hidden="true">{r.emoji}</span> {r.users.length}
          </button>
        );
      })}
      {user && !readOnly && (
        <Popover label="Add reaction" button={<span aria-hidden="true">☺+</span>} className="pl-reaction pl-reaction-add" align="left">
          {(close) => (
            <div className="pl-reaction-picker" role="menu" aria-label="Pick your reaction">
              {REACTIONS.map((r) => (
                <button key={r.content} type="button" role="menuitem" className="pl-reaction-pick" aria-label={r.label}
                  data-pick={r.content} onClick={() => { close(); toggle(r.content); }}>
                  {r.emoji}
                </button>
              ))}
            </div>
          )}
        </Popover>
      )}
      {err && <span className="c-danger small" role="alert">{err}</span>}
    </div>
  );
}

export function confirmAction(msg: string): boolean {
  return typeof window === "undefined" ? true : window.confirm(msg);
}

/** Random GitHub-ish label color (hex without #). */
export function randomColor(): string {
  const palette = ["b60205", "d93f0b", "fbca04", "0e8a16", "006b75", "1d76db", "0052cc", "5319e7", "e99695", "f9d0c4", "fef2c0", "c2e0c6", "bfdadc", "c5def5", "bfd4f2", "d4c5f9"];
  return palette[Math.floor(Math.random() * palette.length)];
}

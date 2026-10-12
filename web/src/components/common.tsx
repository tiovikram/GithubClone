import { useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { errorMessage, isNotFound, type Issue, type Label, type Pull, type User } from "../api";
import { fullDate, labelTextColor, relativeTime } from "../util";
import {
  CheckIcon, CopyIcon, IssueClosedIcon, IssueOpenIcon, MergeIcon, PullClosedIcon, PullIcon,
} from "./icons";

export function Spinner({ label = "Loading…" }: { label?: string }) {
  return (
    <div className="loading" role="status" aria-live="polite">
      <span className="spinner" aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}

export function ErrorBox({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  return (
    <div className="flash flash-error" role="alert">
      <span>{errorMessage(error)}</span>
      {onRetry && (
        <button type="button" className="btn btn-sm" onClick={onRetry}>
          Retry
        </button>
      )}
    </div>
  );
}

export function Flash({ kind = "info", children }: { kind?: "info" | "error" | "success" | "warn"; children: ReactNode }) {
  return (
    <div className={`flash flash-${kind}`} role={kind === "error" ? "alert" : "status"}>
      {children}
    </div>
  );
}

export function Blankslate({ title, children, icon }: { title: string; children?: ReactNode; icon?: ReactNode }) {
  return (
    <div className="blankslate">
      {icon && <div className="blankslate-icon">{icon}</div>}
      <h3>{title}</h3>
      {children && <div className="muted">{children}</div>}
    </div>
  );
}

/**
 * Render loading / error / content. A 404 renders the NotFound view unless `notFound` is given.
 */
export function Async<T>({
  state,
  children,
  notFound,
}: {
  state: { data: T | undefined; error: unknown; loading: boolean; reload: () => void };
  children: (d: T) => ReactNode;
  notFound?: ReactNode;
}) {
  if (state.error) {
    if (isNotFound(state.error)) return <>{notFound ?? <NotFoundView />}</>;
    return <ErrorBox error={state.error} onRetry={state.reload} />;
  }
  if (state.loading && state.data === undefined) return <Spinner />;
  if (state.data === undefined) return <Spinner />;
  return <>{children(state.data)}</>;
}

export function NotFoundView() {
  return (
    <div className="notfound">
      <div className="notfound-code">404</div>
      <h1>This is not the page you are looking for.</h1>
      <p className="muted">The page may not exist, or you may not have permission to see it.</p>
      <Link className="btn" to="/">
        Go home
      </Link>
    </div>
  );
}

export function Avatar({ user, size = 20 }: { user: Pick<User, "login" | "avatar_url"> | null | undefined; size?: number }) {
  const [broken, setBroken] = useState(false);
  if (!user) return <span className="avatar avatar-ghost" style={{ width: size, height: size }} aria-hidden="true" />;
  if (broken || !user.avatar_url) {
    const hue = [...user.login].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 7);
    return (
      <span className="avatar avatar-fallback" aria-hidden="true"
        style={{ width: size, height: size, fontSize: size * 0.5, background: `hsl(${hue} 55% 50%)` }}>
        {user.login[0]?.toUpperCase()}
      </span>
    );
  }
  return (
    <img className="avatar" src={user.avatar_url} alt="" width={size} height={size}
      onError={() => setBroken(true)} loading="lazy" />
  );
}

export function UserLink({ user }: { user: Pick<User, "login"> | null | undefined }) {
  if (!user) return <span className="muted">ghost</span>;
  return (
    <Link className="user-link" to={`/${user.login}`}>
      {user.login}
    </Link>
  );
}

export function RelTime({ iso, prefix }: { iso: string | null | undefined; prefix?: string }) {
  if (!iso) return null;
  return (
    <time dateTime={iso} title={fullDate(iso)}>
      {prefix}
      {relativeTime(iso)}
    </time>
  );
}

export function LabelPill({ label }: { label: Pick<Label, "name" | "color" | "description"> }) {
  const bg = `#${label.color || "ededed"}`;
  return (
    <span className="label-pill" style={{ background: bg, color: labelTextColor(label.color || "ededed") }}
      title={label.description || undefined}>
      {label.name}
    </span>
  );
}

export function StateBadge({ item }: { item: Issue | Pull }) {
  const isPull = "head" in item || !!(item as Issue).pull_request;
  const merged = "merged_at" in item ? !!item.merged_at : !!(item as Issue).pull_request?.merged_at;
  if (isPull) {
    if (merged) return <span className="state state-merged"><MergeIcon /> Merged</span>;
    if (item.state === "closed") return <span className="state state-closed"><PullClosedIcon /> Closed</span>;
    if ((item as Pull).draft) return <span className="state state-draft"><PullIcon /> Draft</span>;
    return <span className="state state-open"><PullIcon /> Open</span>;
  }
  if (item.state === "closed")
    return <span className="state state-done"><IssueClosedIcon /> Closed</span>;
  return <span className="state state-open"><IssueOpenIcon /> Open</span>;
}

export function ItemIcon({ item }: { item: Issue | Pull }) {
  const isPull = "head" in item || !!(item as Issue).pull_request;
  const merged = "merged_at" in item ? !!item.merged_at : !!(item as Issue).pull_request?.merged_at;
  if (isPull) {
    if (merged) return <MergeIcon className="icon c-merged" />;
    if (item.state === "closed") return <PullClosedIcon className="icon c-closed" />;
    return <PullIcon className="icon c-open" />;
  }
  if (item.state === "closed") return <IssueClosedIcon className="icon c-done" />;
  return <IssueOpenIcon className="icon c-open" />;
}

export function Pagination({ page, next, prev, last, onPage }: {
  page: number; next?: number; prev?: number; last?: number; onPage: (p: number) => void;
}) {
  if (!next && !prev) return null;
  return (
    <nav className="pagination" aria-label="Pagination">
      <button type="button" className="btn btn-sm" disabled={!prev} onClick={() => prev && onPage(prev)}>
        ‹ Previous
      </button>
      <span className="muted">
        Page {page}
        {last ? ` of ${last}` : ""}
      </span>
      <button type="button" className="btn btn-sm" disabled={!next} onClick={() => next && onPage(next)}>
        Next ›
      </button>
    </nav>
  );
}

export function CopyButton({ text, label = "Copy to clipboard" }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button type="button" className="btn btn-sm btn-icon" aria-label={label} title={label}
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
        } catch {
          /* clipboard may be unavailable over plain http */
        }
        setDone(true);
        setTimeout(() => setDone(false), 1500);
      }}>
      {done ? <CheckIcon className="icon c-open" /> : <CopyIcon />}
    </button>
  );
}

export function CodeLine({ text, label }: { text: string; label?: string }) {
  return (
    <div className="input-group">
      <input className="input mono" readOnly value={text} aria-label={label || "Command"}
        onFocus={(e) => e.currentTarget.select()} />
      <CopyButton text={text} />
    </div>
  );
}

export function TabNav({ children, label }: { children: ReactNode; label: string }) {
  return (
    <nav className="tabnav" aria-label={label}>
      {children}
    </nav>
  );
}

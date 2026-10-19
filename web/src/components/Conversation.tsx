import { useId, useState, type FormEvent, type ReactNode } from "react";
import { api, errorMessage, type IssueComment, type Label, type User } from "../api";
import { useAuth } from "../auth";
import { Markdown } from "../markdown";
import { Avatar, ErrorBox, LabelPill, RelTime, Spinner, UserLink } from "./common";
import { useAsync } from "../hooks";
import { Link, useLocation } from "react-router-dom";

export function CommentBox({ user, body, created, association, repoPath, header, menu, footer, onSave, renderBody, id, verb = "commented", className }: {
  user: User | null;
  body: string | null;
  created: string;
  association?: string;
  repoPath: string;
  header?: ReactNode;
  /** Header menu (e.g. a "…" dropdown); `startEdit` switches the box into edit mode. */
  menu?: (startEdit: () => void) => ReactNode;
  /** Below the body (e.g. reactions). */
  footer?: ReactNode;
  /** Saves an edited body; enables editing. */
  onSave?: (body: string) => Promise<void>;
  /** Custom body renderer (e.g. suggestions); defaults to Markdown. */
  renderBody?: (body: string | null) => ReactNode;
  id?: string;
  verb?: string;
  className?: string;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(body || "");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const startEdit = () => {
    setDraft(body || "");
    setErr("");
    setEditing(true);
  };
  return (
    <div className={"timeline-item" + (className ? ` ${className}` : "")} id={id}>
      <div className="timeline-avatar"><Avatar user={user} size={40} /></div>
      <div className="comment box">
        <div className="comment-header">
          <span>
            <UserLink user={user} /> <span className="muted">{verb} <RelTime iso={created} /></span>
          </span>
          <span className="spacer" />
          {header}
          {association && association !== "NONE" && (
            <span className="badge">{association.toLowerCase().replace(/_/g, " ")}</span>
          )}
          {menu?.(startEdit)}
        </div>
        {editing && onSave ? (
          <form className="comment-body" aria-label="Edit comment" onSubmit={async (e) => {
            e.preventDefault();
            setBusy(true);
            setErr("");
            try {
              await onSave(draft);
              setEditing(false);
            } catch (ex) {
              setErr(errorMessage(ex));
            } finally {
              setBusy(false);
            }
          }}>
            <MarkdownEditor value={draft} onChange={setDraft} label="Edit comment" repoPath={repoPath} />
            {err && <p className="c-danger small" role="alert">{err}</p>}
            <div className="form-actions">
              <button type="button" className="btn" onClick={() => setEditing(false)}>Cancel</button>
              <button type="submit" className="btn btn-primary" disabled={busy}>{busy ? "Saving…" : "Update comment"}</button>
            </div>
          </form>
        ) : (
          <div className="comment-body">
            {renderBody ? renderBody(body) : <Markdown source={body} comment repoPath={repoPath} />}
          </div>
        )}
        {footer && <div className="comment-footer">{footer}</div>}
      </div>
    </div>
  );
}

export function MarkdownEditor({ value, onChange, label, placeholder, id, rows = 8, required, repoPath }: {
  value: string;
  onChange: (v: string) => void;
  label: string;
  placeholder?: string;
  id?: string;
  rows?: number;
  required?: boolean;
  repoPath?: string;
}) {
  const [tab, setTab] = useState<"write" | "preview">("write");
  const autoId = useId();
  const tid = id || autoId;
  return (
    <div className="md-editor">
      <div className="md-editor-tabs" role="tablist" aria-label={`${label} editor`}>
        <button type="button" role="tab" aria-selected={tab === "write"} onClick={() => setTab("write")}>Write</button>
        <button type="button" role="tab" aria-selected={tab === "preview"} onClick={() => setTab("preview")}>Preview</button>
      </div>
      {tab === "write" ? (
        <>
          <label htmlFor={tid} className="sr-only">{label}</label>
          <textarea id={tid} className="input textarea" rows={rows} value={value} required={required}
            placeholder={placeholder || "Leave a comment"} onChange={(e) => onChange(e.target.value)} />
          <div className="muted small md-hint">Markdown is supported.</div>
        </>
      ) : (
        <div className="md-preview">
          {value.trim() ? <Markdown source={value} comment repoPath={repoPath} /> : <p className="muted">Nothing to preview</p>}
        </div>
      )}
    </div>
  );
}

/** Comment form; `extra` renders additional buttons (e.g. Close issue) that receive the current text. */
export function CommentForm({ onSubmit, extra, repoPath, initial = "", label = "Add a comment", submitText = "Comment" }: {
  onSubmit: (body: string) => Promise<void>;
  extra?: (body: string, reset: () => void) => ReactNode;
  repoPath: string;
  /** Initial text (e.g. a quote reply). */
  initial?: string;
  label?: string;
  submitText?: string;
}) {
  const { user } = useAuth();
  const loc = useLocation();
  const [body, setBody] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  if (!user) {
    return (
      <div className="flash flash-info">
        <Link to={`/login?return_to=${encodeURIComponent(loc.pathname)}`}>Sign in</Link> to join this conversation.
      </div>
    );
  }
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!body.trim()) return;
    setBusy(true);
    setError(undefined);
    try {
      await onSubmit(body);
      setBody("");
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form className="timeline-item" onSubmit={submit} aria-label={label}>
      <div className="timeline-avatar"><Avatar user={user} size={40} /></div>
      <div className="comment box comment-form">
        <MarkdownEditor value={body} onChange={setBody} label="Comment" repoPath={repoPath} />
        {error !== undefined && <ErrorBox error={error} />}
        <div className="form-actions">
          {extra?.(body, () => setBody(""))}
          <button type="submit" className="btn btn-primary" disabled={busy || !body.trim()}>
            {busy ? "Commenting…" : submitText}
          </button>
        </div>
      </div>
    </form>
  );
}

export function useComments(owner: string, name: string, n: number) {
  return useAsync<IssueComment[]>(() => api.issueComments(owner, name, n), [owner, name, n]);
}

/** Sidebar label section; editable when `canEdit`. */
export function LabelsSidebar({ owner, name, number, labels, canEdit, onChange }: {
  owner: string;
  name: string;
  number: number;
  labels: Label[];
  canEdit: boolean;
  onChange: (labels: Label[]) => void;
}) {
  const base = `/${owner}/${name}`;
  const [editing, setEditing] = useState(false);
  const all = useAsync(() => (editing ? api.repoLabels(owner, name) : Promise.resolve([] as Label[])), [owner, name, editing]);
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const start = () => {
    setSel(new Set(labels.map((l) => l.name)));
    setErr("");
    setEditing(true);
  };
  const save = async () => {
    setBusy(true);
    setErr("");
    try {
      const res = await api.setIssueLabels(owner, name, number, [...sel]);
      onChange(res);
      setEditing(false);
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="sidebar-section">
      <div className="sidebar-heading">
        <h3 className="h6">Labels</h3>
        {canEdit && !editing && (
          <button type="button" className="btn-link small" onClick={start}>Edit</button>
        )}
      </div>
      {editing ? (
        <fieldset className="label-editor">
          <legend className="sr-only">Apply labels</legend>
          {all.loading ? <Spinner /> : all.error ? <ErrorBox error={all.error} onRetry={all.reload} /> :
            (all.data || []).length === 0 ? <p className="muted small">This repository has no labels.</p> :
            (all.data || []).map((l) => (
              <label key={l.id} className="check-row">
                <input type="checkbox" checked={sel.has(l.name)} onChange={(e) => {
                  const s = new Set(sel);
                  if (e.target.checked) s.add(l.name); else s.delete(l.name);
                  setSel(s);
                }} />
                <LabelPill label={l} />
              </label>
            ))}
          {err && <p className="c-danger small" role="alert">{err}</p>}
          <Link to={`${base}/labels`} className="small">Edit labels</Link>
          <div className="form-actions">
            <button type="button" className="btn btn-sm" onClick={() => setEditing(false)}>Cancel</button>
            <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={save}>
              {busy ? "Saving…" : "Apply"}
            </button>
          </div>
        </fieldset>
      ) : labels.length ? (
        <div className="label-list">{labels.map((l) => (
          <Link key={l.id} to={`${base}/issues?labels=${encodeURIComponent(l.name)}`} className="label-link"><LabelPill label={l} /></Link>
        ))}</div>
      ) : (
        <p className="muted small">None yet</p>
      )}
    </section>
  );
}

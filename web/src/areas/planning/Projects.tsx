// Projects (v2): lists for users (`/users/{u}/projects`, profile tab), organizations
// (`/orgs/{o}/projects`) and repositories (`/{o}/{r}/projects`); the project page with table and
// board views (`/{users|orgs}/{login}/projects/{n}/views/{v}`), items (issues, pull requests,
// draft issues), field values, and project settings (fields, visibility, linked repositories).
import { useMemo, useState, type DragEvent } from "react";
import { Link, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { errorMessage, type User } from "../../api";
import { useAuth } from "../../auth";
import { Async, Blankslate, ErrorBox, Flash, LabelPill, RelTime, Spinner } from "../../components/common";
import { MarkdownEditor } from "../../components/Conversation";
import { IssueClosedIcon, IssueOpenIcon, MergeIcon, PullClosedIcon, PullIcon } from "../../components/icons";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { papi } from "./api";
import { parseIssueRef } from "./Sidebar";
import {
  OPTION_COLORS, projectHref, projects, type FieldT, type FieldValueT, type ItemT, type OwnerKind, type ProjectSummary,
  type ProjectT, type ViewT,
} from "./projectsApi";
import { confirmAction, Dialog, KebabMenu, Popover } from "./ui";

const TableIcon = () => <span aria-hidden="true">▦</span>;
const BoardIcon = () => <span aria-hidden="true">▥</span>;

// ---------- lists ----------

function ProjectList({ list, emptyTitle, onNew, newLabel = "New project", extra }: {
  list: ProjectSummary[];
  emptyTitle: string;
  onNew?: () => void;
  newLabel?: string;
  extra?: React.ReactNode;
}) {
  const [q, setQ] = useState("");
  const shown = list.filter((p) => !q || p.title.toLowerCase().includes(q.toLowerCase()));
  return (
    <div>
      <div className="toolbar">
        <input className="input input-sm pl-grow-input" placeholder="Search all projects" aria-label="Search all projects" value={q} onChange={(e) => setQ(e.target.value)} />
        {extra}
        {onNew && <button type="button" className="btn btn-primary" onClick={onNew} data-testid="new-project">{newLabel}</button>}
      </div>
      <div className="box">
        {shown.length === 0 ? <Blankslate title={emptyTitle}>Projects help you track issues and pull requests across repositories.</Blankslate> : (
          <ul className="list">
            {shown.map((p) => (
              <li key={p.id} className="list-row pl-project-row" data-testid="project-row" data-title={p.title}>
                <TableIcon />
                <div className="grow">
                  <Link to={projectHref(p)} className="issue-title">{p.title}</Link>{" "}
                  {p.closed && <span className="badge">Closed</span>}{" "}
                  {!p.public && <span className="badge">Private</span>}
                  <div className="muted small">#{p.number} · {p.owner.login} · updated <RelTime iso={p.updatedAt} />{p.shortDescription ? ` · ${p.shortDescription}` : ""}</div>
                </div>
                <span className="muted small">{p.items.totalCount} item{p.items.totalCount === 1 ? "" : "s"}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function NewProjectDialog({ onClose, onCreate }: { onClose: () => void; onCreate: (title: string) => Promise<void> }) {
  const [title, setTitle] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <Dialog title="Create project" onClose={onClose}>
      <form onSubmit={async (e) => {
        e.preventDefault();
        setBusy(true);
        setErr("");
        try { await onCreate(title.trim()); } catch (ex) { setErr(errorMessage(ex)); setBusy(false); }
      }}>
        <div className="form-group">
          <label htmlFor="project-title">Project name</label>
          <input id="project-title" className="input" required autoFocus value={title} onChange={(e) => setTitle(e.target.value)} placeholder="@user's untitled project" />
        </div>
        {err && <p className="c-danger small" role="alert">{err}</p>}
        <div className="form-actions">
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={busy || !title.trim()}>{busy ? "Creating…" : "Create project"}</button>
        </div>
      </form>
    </Dialog>
  );
}

/** Projects of a user or organization (`/users/{u}/projects`, `/orgs/{o}/projects`, profile tab). */
export function OwnerProjects({ login, kind, embedded }: { login: string; kind: OwnerKind; embedded?: boolean }) {
  const [sp, setSp] = useSearchParams();
  const closed = sp.get("query") === "is:closed";
  const st = useAsync(() => projects.ownerProjects(login, true), [login]);
  const nav = useNavigate();
  const { user } = useAuth();
  const [creating, setCreating] = useState(false);
  useTitle(embedded ? "" : `${login} · Projects`);
  const orgs = useAsync(() => (user && user.login === login && kind === "users" ? papi.myOrgs() : Promise.resolve([] as User[])), [user?.login, login, kind]);
  const body = (
    <Async state={st}>
      {(o) => {
        if (!o) return <Blankslate title="Not found" />;
        const list = o.projects.filter((p) => p.closed === closed);
        return (
          <>
            <div className="pl-subnav">
              <button type="button" className={"btn-link" + (!closed ? " pl-on" : "")} onClick={() => { const n = new URLSearchParams(sp); n.delete("query"); setSp(n); }}>
                {o.projects.filter((p) => !p.closed).length} Open
              </button>
              <button type="button" className={"btn-link" + (closed ? " pl-on" : "")} onClick={() => { const n = new URLSearchParams(sp); n.set("query", "is:closed"); setSp(n); }}>
                {o.projects.filter((p) => p.closed).length} Closed
              </button>
            </div>
            <ProjectList list={list} emptyTitle={closed ? "No closed projects" : "No open projects"}
              onNew={o.viewerCanCreateProjects ? () => setCreating(true) : undefined} />
            {!!orgs.data?.length && (
              <p className="small muted pl-org-projects">
                Organization projects: {orgs.data.map((org, i) => <span key={org.login}>{i ? ", " : ""}<Link to={`/orgs/${org.login}/projects`}>{org.login}</Link></span>)}
              </p>
            )}
            {creating && (
              <NewProjectDialog onClose={() => setCreating(false)} onCreate={async (t) => {
                const r = await projects.create(o.id, t);
                nav(`/${kind}/${login}/projects/${r.createProjectV2.projectV2.number}`);
              }} />
            )}
          </>
        );
      }}
    </Async>
  );
  if (embedded) return body;
  return (
    <div className="container pl-page">
      <h1 className="h3 pl-page-title"><Link to={`/${login}`}>{login}</Link> / Projects</h1>
      {body}
    </div>
  );
}

export function UserProjectsPage() {
  const { login = "" } = useParams();
  return <OwnerProjects login={login} kind="users" />;
}
export function OrgProjectsPage() {
  const { org = "" } = useParams();
  return <OwnerProjects login={org} kind="orgs" />;
}

/** `/{o}/{r}/projects`: projects linked to the repository. */
export function RepoProjectsPage() {
  const { repo, owner, name } = useRepo();
  useTitle(`Projects · ${repo.full_name}`);
  const st = useAsync(() => projects.repoProjects(owner, name), [owner, name]);
  const nav = useNavigate();
  const { user } = useAuth();
  const [creating, setCreating] = useState(false);
  const [linking, setLinking] = useState(false);
  const mine = useAsync(async () => {
    if (!linking || !user) return [] as ProjectSummary[];
    const [a, b] = await Promise.all([projects.ownerProjects(owner, false), owner === user.login ? Promise.resolve(null) : projects.ownerProjects(user.login, false)]);
    return [...(a?.projects || []), ...(b?.projects || [])];
  }, [linking, owner, user?.login]);
  const [err, setErr] = useState("");
  return (
    <Async state={st}>
      {(r) => r && (
        <>
          {err && <ErrorBox error={err} />}
          <ProjectList list={r.projectsV2.nodes} emptyTitle="No projects linked to this repository"
            onNew={r.viewerCanCreateProjects ? () => setCreating(true) : undefined}
            extra={r.viewerCanCreateProjects && (
              <Popover label="Link a project" button={<>Link a project ▾</>} testid="link-project" onOpen={() => setLinking(true)}>
                {(close) => mine.loading ? <Spinner /> : (
                  <div className="pl-menu" role="menu">
                    {(mine.data || []).filter((p) => !r.projectsV2.nodes.some((x) => x.id === p.id)).map((p) => (
                      <button key={p.id} type="button" role="menuitem" className="menu-item" onClick={async () => {
                        close();
                        try { await projects.linkRepo(p.id, r.id, true); st.reload(); } catch (e) { setErr(errorMessage(e)); }
                      }}>{p.title}</button>
                    ))}
                    {!(mine.data || []).length && <div className="muted small pl-pad">No projects to link</div>}
                  </div>
                )}
              </Popover>
            )} />
          {creating && (
            <NewProjectDialog onClose={() => setCreating(false)} onCreate={async (t) => {
              const res = await projects.create(r.owner.id, t, r.id);
              nav(`/${r.owner.__typename === "Organization" ? "orgs" : "users"}/${r.owner.login}/projects/${res.createProjectV2.projectV2.number}`);
            }} />
          )}
        </>
      )}
    </Async>
  );
}

// ---------- the project page ----------

function valueOf(item: ItemT, field: FieldT): FieldValueT | undefined {
  return item.fieldValues.nodes.find((v) => v.field?.id === field.id);
}

function itemTitle(item: ItemT): string {
  return item.content?.title || "(redacted)";
}

function ItemIconFor({ item }: { item: ItemT }) {
  const c = item.content;
  if (!c || c.__typename === "DraftIssue") return <span className="pl-draft-icon" aria-label="Draft issue">◌</span>;
  if (c.__typename === "PullRequest") {
    if (c.merged) return <MergeIcon className="icon c-merged" />;
    return c.state === "CLOSED" ? <PullClosedIcon className="icon c-closed" /> : <PullIcon className="icon c-open" />;
  }
  return c.state === "CLOSED" ? <IssueClosedIcon className="icon c-done" /> : <IssueOpenIcon className="icon c-open" />;
}

function contentHref(item: ItemT): string | null {
  const c = item.content;
  if (!c || c.__typename === "DraftIssue" || !c.repository) return null;
  return `/${c.repository.nameWithOwner}/${c.__typename === "PullRequest" ? "pull" : "issues"}/${c.number}`;
}

function OptionPill({ name, color }: { name: string; color?: string }) {
  const c = OPTION_COLORS[color || "GRAY"] || OPTION_COLORS.GRAY;
  return <span className="pl-option" style={{ borderColor: c, color: c }}>{name}</span>;
}

/** Editor for one field value of an item (table cell / item dialog). */
function FieldEditor({ project, item, field, onChange, canEdit }: {
  project: ProjectT; item: ItemT; field: FieldT; onChange: () => void; canEdit: boolean;
}) {
  const v = valueOf(item, field);
  const [editing, setEditing] = useState(false);
  const [err, setErr] = useState("");
  const set = async (value: Record<string, unknown> | null) => {
    setErr("");
    try {
      if (value === null) await projects.clearValue(project.id, item.id, field.id);
      else await projects.setValue(project.id, item.id, field.id, value);
      setEditing(false);
      onChange();
    } catch (e) {
      setErr(errorMessage(e));
    }
  };
  const label = `${field.name} for ${itemTitle(item)}`;
  if (field.dataType === "SINGLE_SELECT") {
    return (
      <>
        <select className="input input-sm pl-cell-input" aria-label={label} disabled={!canEdit} value={v?.optionId || ""}
          onChange={(e) => set(e.target.value ? { singleSelectOptionId: e.target.value } : null)}>
          <option value="">—</option>
          {(field.options || []).map((o) => <option key={o.id} value={o.id}>{o.name}</option>)}
        </select>
        {err && <span className="c-danger small" role="alert">{err}</span>}
      </>
    );
  }
  if (field.dataType === "ITERATION") {
    const its = [...(field.configuration?.iterations || []), ...(field.configuration?.completedIterations || [])];
    return (
      <>
        <select className="input input-sm pl-cell-input" aria-label={label} disabled={!canEdit} value={v?.iterationId || ""}
          onChange={(e) => set(e.target.value ? { iterationId: e.target.value } : null)}>
          <option value="">—</option>
          {its.map((o) => <option key={o.id} value={o.id}>{o.title} ({o.startDate})</option>)}
        </select>
        {err && <span className="c-danger small" role="alert">{err}</span>}
      </>
    );
  }
  if (["TEXT", "NUMBER", "DATE"].includes(field.dataType)) {
    const shown = v?.text ?? (v?.number !== undefined ? String(v.number) : undefined) ?? v?.date;
    if (!editing || !canEdit) {
      return (
        <button type="button" className="pl-cell-btn" aria-label={`Edit ${label}`} disabled={!canEdit} onClick={() => setEditing(true)}>
          {shown ?? <span className="muted">—</span>}
        </button>
      );
    }
    return (
      <form className="pl-cell-form" onSubmit={(e) => {
        e.preventDefault();
        const raw = (new FormData(e.currentTarget).get("v") as string).trim();
        if (!raw) return set(null);
        if (field.dataType === "NUMBER") return set({ number: Number(raw) });
        if (field.dataType === "DATE") return set({ date: raw });
        return set({ text: raw });
      }}>
        <input name="v" className="input input-sm pl-cell-input" aria-label={label} autoFocus defaultValue={shown || ""}
          type={field.dataType === "NUMBER" ? "number" : field.dataType === "DATE" ? "date" : "text"} step="any"
          onBlur={(e) => e.currentTarget.form?.requestSubmit()} onKeyDown={(e) => e.key === "Escape" && setEditing(false)} />
        {err && <span className="c-danger small" role="alert">{err}</span>}
      </form>
    );
  }
  // Built-in fields derived from content.
  const c = item.content;
  if (field.dataType === "ASSIGNEES") return <span className="small">{(c?.assignees?.nodes || []).map((a) => a.login).join(", ") || <span className="muted">—</span>}</span>;
  if (field.dataType === "LABELS") return <span className="label-list">{(c?.labels?.nodes || []).map((l) => <LabelPill key={l.name} label={{ ...l, description: null }} />)}</span>;
  if (field.dataType === "REPOSITORY") return <span className="small">{c?.repository?.nameWithOwner || ""}</span>;
  return <span className="muted small">{v?.text || v?.name || ""}</span>;
}

/** Columns shown in table views: Title, Assignees, Status, then custom fields. */
function tableFields(fields: FieldT[]): FieldT[] {
  const builtin = ["ASSIGNEES", "SINGLE_SELECT", "TEXT", "NUMBER", "DATE", "ITERATION", "LABELS"];
  return fields.filter((f) => f.dataType !== "TITLE" && builtin.includes(f.dataType));
}

function AddItemRow({ project, onAdded }: { project: ProjectT; onAdded: () => void }) {
  const [v, setV] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const owner = project.owner.login;
  return (
    <form className="pl-add-item" onSubmit={async (e) => {
      e.preventDefault();
      const t = v.trim();
      if (!t) return;
      setBusy(true);
      setErr("");
      try {
        // owner/repo#123 or an issue URL adds that issue / pull request; anything else is a draft.
        const ref = parseIssueRef(t, owner, "");
        if (ref && ref.repo) {
          const i = await papi.issue(ref.owner, ref.repo, ref.number);
          if (!i.node_id) throw new Error("Issue has no node id");
          await projects.addContent(project.id, i.node_id);
        } else {
          await projects.addDraft(project.id, t);
        }
        setV("");
        onAdded();
      } catch (ex) {
        setErr(errorMessage(ex));
      } finally {
        setBusy(false);
      }
    }}>
      <label htmlFor="add-item" className="sr-only">Add item</label>
      <input id="add-item" className="input input-sm" placeholder="+ Add item: type a title for a draft, or owner/repo#123" value={v}
        onChange={(e) => setV(e.target.value)} disabled={busy} />
      <button type="submit" className="btn btn-sm" disabled={busy || !v.trim()}>Add</button>
      {err && <span className="c-danger small" role="alert">{err}</span>}
    </form>
  );
}

function ItemDialog({ project, item, fields, onClose, onChange, canEdit }: {
  project: ProjectT; item: ItemT; fields: FieldT[]; onClose: () => void; onChange: () => void; canEdit: boolean;
}) {
  const c = item.content;
  const draft = c?.__typename === "DraftIssue";
  const [title, setTitle] = useState(c?.title || "");
  const [body, setBody] = useState(c?.body || "");
  const [editing, setEditing] = useState(false);
  const [err, setErr] = useState("");
  const href = contentHref(item);
  return (
    <Dialog title={itemTitle(item)} onClose={onClose} wide>
      <div className="pl-item-dialog">
        <div className="pl-item-main">
          {draft ? (
            editing ? (
              <form onSubmit={async (e) => {
                e.preventDefault();
                try { await projects.updateDraft(c!.id, title, body); setEditing(false); onChange(); } catch (ex) { setErr(errorMessage(ex)); }
              }}>
                <div className="form-group"><label htmlFor="draft-title">Title</label>
                  <input id="draft-title" className="input" value={title} onChange={(e) => setTitle(e.target.value)} required /></div>
                <MarkdownEditor value={body} onChange={setBody} label="Body" rows={6} />
                <div className="form-actions">
                  <button type="button" className="btn" onClick={() => setEditing(false)}>Cancel</button>
                  <button type="submit" className="btn btn-primary">Save</button>
                </div>
              </form>
            ) : (
              <>
                <p className="muted small">Draft issue</p>
                <Markdown source={c?.body} comment />
                {canEdit && <button type="button" className="btn btn-sm" onClick={() => setEditing(true)} data-testid="edit-draft">Edit</button>}
              </>
            )
          ) : (
            <>
              <p className="small"><ItemIconFor item={item} /> {c?.repository?.nameWithOwner}#{c?.number}</p>
              {href && <Link to={href} className="btn btn-sm" onClick={onClose}>Open {c?.__typename === "PullRequest" ? "pull request" : "issue"}</Link>}
            </>
          )}
          {err && <p className="c-danger small" role="alert">{err}</p>}
        </div>
        <div className="pl-item-fields">
          {fields.filter((f) => !["TITLE", "LINKED_PULL_REQUESTS", "REVIEWERS", "TRACKS", "TRACKED_BY", "PARENT_ISSUE", "SUB_ISSUES_PROGRESS", "MILESTONE"].includes(f.dataType)).map((f) => (
            <div key={f.id} className="pl-item-field">
              <div className="muted small">{f.name}</div>
              <FieldEditor project={project} item={item} field={f} onChange={onChange} canEdit={canEdit} />
            </div>
          ))}
        </div>
      </div>
    </Dialog>
  );
}

function TableView({ project, items, fields, canEdit, reload, open }: {
  project: ProjectT; items: ItemT[]; fields: FieldT[]; canEdit: boolean; reload: () => void; open: (i: ItemT) => void;
}) {
  const cols = tableFields(fields);
  return (
    <div className="pl-table-wrap">
      <table className="pl-table" data-testid="project-table">
        <thead>
          <tr>
            <th className="pl-col-num">#</th>
            <th>Title</th>
            {cols.map((f) => <th key={f.id}>{f.name}</th>)}
            <th className="sr-only">Actions</th>
          </tr>
        </thead>
        <tbody>
          {items.map((it, i) => (
            <tr key={it.id} data-testid="project-item" data-title={itemTitle(it)}>
              <td className="pl-col-num muted">{i + 1}</td>
              <td className="pl-col-title">
                <ItemIconFor item={it} />{" "}
                <button type="button" className="btn-link pl-item-title" onClick={() => open(it)}>{itemTitle(it)}</button>
                {it.content?.number && <span className="muted small"> #{it.content.number}</span>}
              </td>
              {cols.map((f) => (
                <td key={f.id}><FieldEditor project={project} item={it} field={f} onChange={reload} canEdit={canEdit} /></td>
              ))}
              <td>
                {canEdit && <ItemMenu project={project} item={it} reload={reload} />}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ItemMenu({ project, item, reload }: { project: ProjectT; item: ItemT; reload: () => void }) {
  return (
    <KebabMenu label={`Actions for ${itemTitle(item)}`} items={[
      { label: item.isArchived ? "Restore" : "Archive", onClick: async () => { await projects.archive(project.id, item.id, !item.isArchived); reload(); } },
      { label: "Delete from project", danger: true, onClick: async () => {
        if (confirmAction("Delete this item from the project?")) { await projects.deleteItem(project.id, item.id); reload(); }
      } },
    ]} />
  );
}

function BoardView({ project, items, field, canEdit, reload, open }: {
  project: ProjectT; items: ItemT[]; field: FieldT | undefined; canEdit: boolean; reload: () => void; open: (i: ItemT) => void;
}) {
  const [over, setOver] = useState<string | null>(null);
  const [err, setErr] = useState("");
  if (!field) return <Flash>Add a single select field (like Status) to use the board layout.</Flash>;
  const columns = [{ id: "", name: `No ${field.name}`, color: "GRAY", description: "" }, ...(field.options || [])];
  const move = async (itemId: string, optionId: string) => {
    setErr("");
    try {
      if (optionId) await projects.setValue(project.id, itemId, field.id, { singleSelectOptionId: optionId });
      else await projects.clearValue(project.id, itemId, field.id);
      reload();
    } catch (e) {
      setErr(errorMessage(e));
    }
  };
  return (
    <>
      {err && <ErrorBox error={err} />}
      <div className="pl-board" data-testid="project-board">
        {columns.map((col) => {
          const colItems = items.filter((it) => (valueOf(it, field)?.optionId || "") === col.id);
          return (
            <section key={col.id || "none"} className={"pl-board-col" + (over === col.id ? " pl-drop" : "")} aria-label={col.name}
              data-testid="board-column" data-column={col.name}
              onDragOver={(e: DragEvent) => { if (canEdit) { e.preventDefault(); setOver(col.id); } }}
              onDragLeave={() => setOver(null)}
              onDrop={(e: DragEvent) => { e.preventDefault(); setOver(null); const id = e.dataTransfer.getData("text/plain"); if (id) move(id, col.id); }}>
              <header className="pl-board-head">
                <span className="pl-dot-color" style={{ background: OPTION_COLORS[col.color] || OPTION_COLORS.GRAY }} />
                <strong>{col.name}</strong> <span className="counter">{colItems.length}</span>
              </header>
              {col.description && <p className="muted small pl-board-desc">{col.description}</p>}
              <div className="pl-board-cards">
                {colItems.map((it) => (
                  <article key={it.id} className="pl-card box" draggable={canEdit} data-testid="board-card" data-title={itemTitle(it)}
                    onDragStart={(e: DragEvent) => e.dataTransfer.setData("text/plain", it.id)}>
                    <div className="small muted">{it.content?.repository ? `${it.content.repository.nameWithOwner} #${it.content.number}` : "Draft"}</div>
                    <div className="pl-card-title"><ItemIconFor item={it} /> <button type="button" className="btn-link pl-item-title" onClick={() => open(it)}>{itemTitle(it)}</button></div>
                    {canEdit && (
                      <div className="pl-card-foot">
                        <label className="sr-only" htmlFor={`move-${it.id}`}>Move {itemTitle(it)}</label>
                        <select id={`move-${it.id}`} className="input input-sm" value={col.id} onChange={(e) => move(it.id, e.target.value)} data-testid="card-move">
                          {columns.map((c) => <option key={c.id || "none"} value={c.id}>{c.name}</option>)}
                        </select>
                        <ItemMenu project={project} item={it} reload={reload} />
                      </div>
                    )}
                  </article>
                ))}
              </div>
            </section>
          );
        })}
      </div>
    </>
  );
}

function NewViewMenu({ project, kind, onCreated }: { project: ProjectT; kind: OwnerKind; onCreated: (n: number) => void }) {
  const [err, setErr] = useState("");
  const create = async (layout: "table" | "board") => {
    setErr("");
    try {
      const dbId = kind === "users" ? await projects.ownerDbId(project.owner.login) : 0;
      const n = project.views.nodes.length + 1;
      const v = await projects.createView(kind, project.owner.login, dbId, project.number, { name: `View ${n}`, layout });
      onCreated(v.number);
    } catch (e) {
      setErr(errorMessage(e));
    }
  };
  return (
    <>
      <Popover label="New view" button={<>+ New view</>} className="btn-link pl-view-tab" testid="new-view" align="left">
        {(close) => (
          <div className="pl-menu" role="menu">
            <button type="button" role="menuitem" className="menu-item" onClick={() => { close(); create("table"); }}><TableIcon /> Table</button>
            <button type="button" role="menuitem" className="menu-item" onClick={() => { close(); create("board"); }}><BoardIcon /> Board</button>
          </div>
        )}
      </Popover>
      {err && <span className="c-danger small" role="alert">{err}</span>}
    </>
  );
}

function useProject(kind: OwnerKind) {
  const params = useParams();
  const login = (kind === "orgs" ? params.org : params.login) || "";
  const number = Number(params.number);
  return { login, number, base: `/${kind}/${login}/projects/${number}` };
}

export function ProjectPage({ kind }: { kind: OwnerKind }) {
  const { login, number, base } = useProject(kind);
  const vn = Number(useParams().view || 1);
  const [sp, setSp] = useSearchParams();
  const showArchived = sp.get("archived") === "1";
  const nav = useNavigate();
  const meta = useAsync(() => projects.project(login, number), [login, number]);
  const view: ViewT | undefined = meta.data?.views.nodes.find((v) => v.number === vn) || meta.data?.views.nodes[0];
  const filter = view?.filter || null;
  const st = useAsync(() => (meta.data ? (filter ? projects.project(login, number, filter) : Promise.resolve(meta.data)) : Promise.resolve(null)), [meta.data, filter]);
  const [openItem, setOpenItem] = useState<string | null>(null);
  useTitle(meta.data ? `${meta.data.title} · ${view?.name || ""}` : "Project");
  const reload = () => meta.reload();
  const data = st.data || meta.data;
  const items = useMemo(() => (data?.items.nodes || []).filter((i) => showArchived === i.isArchived), [data, showArchived]);
  if (meta.data === null && !meta.loading) return <div className="container"><Blankslate title="Project not found" /></div>;
  return (
    <div className="container pl-page">
      <Async state={meta}>
        {(p) => {
          if (!p) return <Blankslate title="Project not found" />;
          const canEdit = p.viewerCanUpdate;
          const fields = p.fields.nodes;
          const groupField = fields.find((f) => f.id === view?.verticalGroupByFields.nodes[0]?.id) || fields.find((f) => f.dataType === "SINGLE_SELECT" && f.name === "Status") || fields.find((f) => f.dataType === "SINGLE_SELECT");
          const current = openItem ? (data?.items.nodes || []).find((i) => i.id === openItem) : undefined;
          return (
            <>
              <div className="pl-project-head">
                <h1 className="h3" data-testid="project-title">{p.title}</h1>
                {p.closed && <span className="badge">Closed</span>}
                {!p.public && <span className="badge">Private</span>}
                <span className="spacer" />
                <Link to={`/${kind}/${login}/projects`} className="btn btn-sm">All projects</Link>
                {canEdit && <Link to={`${base}/settings`} className="btn btn-sm" data-testid="project-settings">Settings</Link>}
              </div>
              {p.shortDescription && <p className="muted">{p.shortDescription}</p>}
              <nav className="pl-views" aria-label="Project views">
                {p.views.nodes.map((v) => (
                  <Link key={v.number} to={`${base}/views/${v.number}`} className={"pl-view-tab" + (v.number === view?.number ? " selected" : "")}
                    data-testid={`view-tab-${v.number}`} aria-current={v.number === view?.number ? "page" : undefined}>
                    {v.layout === "BOARD_LAYOUT" ? <BoardIcon /> : <TableIcon />} {v.name}
                  </Link>
                ))}
                {canEdit && <NewViewMenu project={p} kind={kind} onCreated={(n) => { meta.reload(); nav(`${base}/views/${n}`); }} />}
              </nav>
              <div className="pl-view-bar small">
                {view?.filter && <span className="muted">Filter: <code>{view.filter}</code></span>}
                <span className="spacer" />
                <label className="check-row"><input type="checkbox" checked={showArchived}
                  onChange={(e) => { const n = new URLSearchParams(sp); if (e.target.checked) n.set("archived", "1"); else n.delete("archived"); setSp(n); }} /> Archived items</label>
              </div>
              {st.loading && !st.data ? <Spinner /> : view?.layout === "BOARD_LAYOUT" ? (
                <BoardView project={p} items={items} field={groupField} canEdit={canEdit && !showArchived} reload={reload} open={(i) => setOpenItem(i.id)} />
              ) : (
                <TableView project={p} items={items} fields={fields} canEdit={canEdit} reload={reload} open={(i) => setOpenItem(i.id)} />
              )}
              {items.length === 0 && <p className="muted small pl-pad">{showArchived ? "No archived items." : "This view has no items yet."}</p>}
              {canEdit && !showArchived && <AddItemRow project={p} onAdded={reload} />}
              {p.readme && (
                <div className="box box-body pl-readme"><h2 className="h5">README</h2><Markdown source={p.readme} comment /></div>
              )}
              {current && <ItemDialog project={p} item={current} fields={fields} canEdit={canEdit} onClose={() => setOpenItem(null)} onChange={reload} />}
            </>
          );
        }}
      </Async>
    </div>
  );
}

const COLORS = Object.keys(OPTION_COLORS);

function NewFieldForm({ project, onDone }: { project: ProjectT; onDone: () => void }) {
  const [name, setName] = useState("");
  const [type, setType] = useState("TEXT");
  const [options, setOptions] = useState("");
  const [duration, setDuration] = useState(14);
  const [start, setStart] = useState(new Date().toISOString().slice(0, 10));
  const [err, setErr] = useState("");
  return (
    <form className="box box-body pl-field-form" aria-label="New field" onSubmit={async (e) => {
      e.preventDefault();
      setErr("");
      try {
        const extra: Record<string, unknown> = {};
        if (type === "SINGLE_SELECT") {
          extra.singleSelectOptions = options.split(",").map((o) => o.trim()).filter(Boolean)
            .map((o, i) => ({ name: o, color: COLORS[i % COLORS.length], description: "" }));
        }
        if (type === "ITERATION") extra.iterationConfiguration = { duration, startDate: start };
        await projects.createField(project.id, type, name.trim(), extra);
        setName("");
        setOptions("");
        onDone();
      } catch (ex) {
        setErr(errorMessage(ex));
      }
    }}>
      <div className="form-row">
        <div className="form-group">
          <label htmlFor="field-name">Field name</label>
          <input id="field-name" className="input" required value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="field-type">Field type</label>
          <select id="field-type" className="input" value={type} onChange={(e) => setType(e.target.value)}>
            <option value="TEXT">Text</option>
            <option value="NUMBER">Number</option>
            <option value="DATE">Date</option>
            <option value="SINGLE_SELECT">Single select</option>
            <option value="ITERATION">Iteration</option>
          </select>
        </div>
      </div>
      {type === "SINGLE_SELECT" && (
        <div className="form-group">
          <label htmlFor="field-options">Options (comma-separated)</label>
          <input id="field-options" className="input" required value={options} onChange={(e) => setOptions(e.target.value)} placeholder="Low, Medium, High" />
        </div>
      )}
      {type === "ITERATION" && (
        <div className="form-row">
          <div className="form-group">
            <label htmlFor="field-start">Starts on</label>
            <input id="field-start" type="date" className="input" value={start} onChange={(e) => setStart(e.target.value)} />
          </div>
          <div className="form-group">
            <label htmlFor="field-duration">Duration (days)</label>
            <input id="field-duration" type="number" min={1} className="input" value={duration} onChange={(e) => setDuration(Number(e.target.value))} />
          </div>
        </div>
      )}
      {err && <p className="c-danger small" role="alert">{err}</p>}
      <div className="form-actions"><button type="submit" className="btn btn-primary" disabled={!name.trim()}>Save field</button></div>
    </form>
  );
}

export function ProjectSettingsPage({ kind }: { kind: OwnerKind }) {
  const { login, number, base } = useProject(kind);
  const st = useAsync(() => projects.project(login, number), [login, number]);
  const nav = useNavigate();
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [repoInput, setRepoInput] = useState("");
  useTitle("Project settings");
  return (
    <div className="container pl-page">
      <Async state={st}>
        {(p) => {
          if (!p) return <Blankslate title="Project not found" />;
          if (!p.viewerCanUpdate) return <Navigate to={base} replace />;
          const save = async (patch: Parameters<typeof projects.update>[1], text = "Saved.") => {
            setErr("");
            setMsg("");
            try { await projects.update(p.id, patch); setMsg(text); st.reload(); } catch (e) { setErr(errorMessage(e)); }
          };
          return (
            <div className="pl-settings">
              <div className="pl-project-head">
                <h1 className="h3"><Link to={base}>{p.title}</Link> / Settings</h1>
              </div>
              {msg && <Flash kind="success">{msg}</Flash>}
              {err && <Flash kind="error">{err}</Flash>}
              <form className="box box-body" aria-label="Project settings" onSubmit={(e) => {
                e.preventDefault();
                const f = new FormData(e.currentTarget);
                save({ title: String(f.get("title")), shortDescription: String(f.get("desc")), readme: String(f.get("readme")) });
              }}>
                <div className="form-group"><label htmlFor="ps-title">Project name</label>
                  <input id="ps-title" name="title" className="input" required defaultValue={p.title} /></div>
                <div className="form-group"><label htmlFor="ps-desc">Short description</label>
                  <input id="ps-desc" name="desc" className="input" defaultValue={p.shortDescription || ""} /></div>
                <div className="form-group"><label htmlFor="ps-readme">README</label>
                  <textarea id="ps-readme" name="readme" className="input textarea" rows={5} defaultValue={p.readme || ""} /></div>
                <div className="form-actions"><button type="submit" className="btn btn-primary">Save changes</button></div>
              </form>

              <h2 className="h4 pl-section-title">Custom fields</h2>
              <div className="box">
                <ul className="list">
                  {p.fields.nodes.map((f) => (
                    <li key={f.id} className="list-row" data-testid="field-row" data-field={f.name}>
                      <div className="grow"><strong>{f.name}</strong> <span className="muted small">{f.dataType.toLowerCase().replace(/_/g, " ")}</span>
                        {f.options && <div className="small">{f.options.map((o) => <OptionPill key={o.id} name={o.name} color={o.color} />)}</div>}
                        {f.configuration && <div className="muted small">{f.configuration.duration}-day iterations: {f.configuration.iterations.map((i) => i.title).join(", ")}</div>}
                      </div>
                      {["TEXT", "NUMBER", "DATE", "SINGLE_SELECT", "ITERATION"].includes(f.dataType) && f.name !== "Status" && (
                        <button type="button" className="btn btn-sm btn-danger-outline" onClick={async () => {
                          if (!confirmAction(`Delete field "${f.name}"?`)) return;
                          try { await projects.deleteField(f.id); st.reload(); } catch (e) { setErr(errorMessage(e)); }
                        }}>Delete</button>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
              <NewFieldForm project={p} onDone={() => st.reload()} />

              <h2 className="h4 pl-section-title">Linked repositories</h2>
              <div className="box">
                <ul className="list">
                  {p.repositories.nodes.map((r) => (
                    <li key={r.id} className="list-row"><Link className="grow" to={`/${r.nameWithOwner}`}>{r.nameWithOwner}</Link>
                      <button type="button" className="btn btn-sm" onClick={async () => { try { await projects.linkRepo(p.id, r.id, false); st.reload(); } catch (e) { setErr(errorMessage(e)); } }}>Unlink</button>
                    </li>
                  ))}
                  {!p.repositories.nodes.length && <li className="list-row muted small">No linked repositories</li>}
                </ul>
                <form className="box-body pl-inline-form" onSubmit={async (e) => {
                  e.preventDefault();
                  setErr("");
                  try {
                    const [o, r] = repoInput.trim().split("/");
                    const d = await projects.repoProjects(o, r);
                    if (!d) throw new Error("Repository not found");
                    await projects.linkRepo(p.id, d.id, true);
                    setRepoInput("");
                    st.reload();
                  } catch (ex) { setErr(errorMessage(ex)); }
                }}>
                  <label htmlFor="link-repo" className="sr-only">Repository to link</label>
                  <input id="link-repo" className="input input-sm" placeholder="owner/repo" value={repoInput} onChange={(e) => setRepoInput(e.target.value)} />
                  <button type="submit" className="btn btn-sm" disabled={!repoInput.includes("/")}>Link repository</button>
                </form>
              </div>

              <h2 className="h4 pl-section-title">Danger zone</h2>
              <div className="box danger-zone">
                <div className="list-row">
                  <div className="grow"><strong>Visibility</strong><div className="muted small">This project is currently {p.public ? "public" : "private"}.</div></div>
                  <button type="button" className="btn btn-sm btn-danger-outline" onClick={() => save({ public: !p.public }, `Project is now ${p.public ? "private" : "public"}.`)}>
                    Make {p.public ? "private" : "public"}
                  </button>
                </div>
                <div className="list-row">
                  <div className="grow"><strong>{p.closed ? "Re-open project" : "Close project"}</strong><div className="muted small">Closing a project hides it from the list of open projects.</div></div>
                  <button type="button" className="btn btn-sm btn-danger-outline" data-testid="close-project" onClick={() => save({ closed: !p.closed }, p.closed ? "Project re-opened." : "Project closed.")}>
                    {p.closed ? "Re-open this project" : "Close this project"}
                  </button>
                </div>
                <div className="list-row">
                  <div className="grow"><strong>Delete project</strong><div className="muted small">Once you delete a project, there is no going back.</div></div>
                  <button type="button" className="btn btn-sm btn-danger" data-testid="delete-project" onClick={async () => {
                    if (!confirmAction("Delete this project? This cannot be undone.")) return;
                    try { await projects.remove(p.id); nav(`/${kind}/${login}/projects`); } catch (e) { setErr(errorMessage(e)); }
                  }}>Delete this project</button>
                </div>
              </div>
            </div>
          );
        }}
      </Async>
    </div>
  );
}

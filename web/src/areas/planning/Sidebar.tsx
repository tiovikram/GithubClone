// Issue / pull request sidebar sections: assignees, reviewers, milestone, projects,
// relationships (parent, sub-issues, blocked by / blocking), lock, pin, transfer, delete.
import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, errorMessage, type Repo, type User } from "../../api";
import { useAuth } from "../../auth";
import { Avatar, ErrorBox, Spinner, UserLink } from "../../components/common";
import { CheckIcon, CommentIcon, IssueClosedIcon, IssueOpenIcon, LockIcon } from "../../components/icons";
import { useAsync } from "../../hooks";
import { gql, papi, type FullIssue, type FullPull, type FullReview, type Milestone } from "./api";
import { Dialog, Popover, Progress, SelectMenu, userOption } from "./ui";

const Gear = () => <span aria-hidden="true" className="pl-gear">⚙</span>;

function SectionHead({ title, edit }: { title: string; edit?: React.ReactNode }) {
  return (
    <div className="sidebar-heading">
      <h3 className="h6">{title}</h3>
      {edit}
    </div>
  );
}

// ---------- assignees ----------

export function AssigneesSection({ owner, name, number, assignees, canEdit, onChange }: {
  owner: string; name: string; number: number; assignees: User[]; canEdit: boolean; onChange: (a: User[]) => void;
}) {
  const { user } = useAuth();
  const [open, setOpen] = useState(false);
  const all = useAsync(() => (open ? papi.assignees(owner, name) : Promise.resolve([] as User[])), [owner, name, open]);
  const current = assignees.map((a) => a.login);
  const apply = async (keys: string[]) => {
    const add = keys.filter((k) => !current.includes(k));
    const remove = current.filter((k) => !keys.includes(k));
    let res: FullIssue | null = null;
    if (remove.length) res = await papi.removeAssignees(owner, name, number, remove);
    if (add.length) res = await papi.addAssignees(owner, name, number, add);
    if (res) onChange(res.assignees || []);
  };
  return (
    <section className="sidebar-section" data-testid="sidebar-assignees">
      <SectionHead title="Assignees" edit={canEdit && (
        <Popover label="Edit assignees" onOpen={() => setOpen(true)} button={<Gear />} className="btn-link btn-icon" testid="edit-assignees">
          {(close) => {
            return (
              <SelectMenu title="Assign up to 10 people to this issue" multi loading={all.loading}
                options={(all.data || []).map(userOption)} selected={current} filterPlaceholder="Filter assignees"
                onApply={async (k) => { await apply(k); close(); }} />
            );
          }}
        </Popover>
      )} />
      {assignees.length ? (
        <ul className="pl-people">
          {assignees.map((a) => <li key={a.login}><Avatar user={a} size={20} /> <UserLink user={a} /></li>)}
        </ul>
      ) : (
        <p className="muted small">
          No one{canEdit && user && !current.includes(user.login) ? <> — <button type="button" className="btn-link" onClick={() => apply([user.login])}>assign yourself</button></> : " assigned"}
        </p>
      )}
    </section>
  );
}

// ---------- milestone ----------

export function MilestoneSection({ owner, name, milestone, canEdit, onSet }: {
  owner: string; name: string; milestone: Milestone | null | undefined; canEdit: boolean;
  onSet: (n: number | null) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const all = useAsync(() => (open ? papi.milestones(owner, name, "open") : Promise.resolve([] as Milestone[])), [owner, name, open]);
  const base = `/${owner}/${name}`;
  return (
    <section className="sidebar-section" data-testid="sidebar-milestone">
      <SectionHead title="Milestone" edit={canEdit && (
        <Popover label="Edit milestone" onOpen={() => setOpen(true)} button={<Gear />} className="btn-link btn-icon" testid="edit-milestone">
          {(close) => {
            return (
              <SelectMenu title="Set milestone" loading={all.loading} filterPlaceholder="Filter milestones"
                options={(all.data || []).map((m) => ({ key: String(m.number), text: m.title, label: m.title,
                  description: m.due_on ? `Due by ${new Date(m.due_on).toLocaleDateString()}` : "No due date" }))}
                selected={milestone ? [String(milestone.number)] : []}
                onApply={async (k) => { await onSet(k.length ? Number(k[0]) : null); close(); }}
                footer={<Link to={`${base}/milestones`} className="small">View all milestones</Link>} />
            );
          }}
        </Popover>
      )} />
      {milestone ? (
        <div>
          <Progress done={milestone.closed_issues} total={milestone.open_issues + milestone.closed_issues} />
          <Link to={`${base}/milestone/${milestone.number}`} className="pl-strong-link">{milestone.title}</Link>
        </div>
      ) : <p className="muted small">No milestone</p>}
    </section>
  );
}

// ---------- reviewers (pull requests) ----------

const REVIEW_ICON: Record<string, React.ReactNode> = {
  APPROVED: <CheckIcon className="icon c-open" />,
  CHANGES_REQUESTED: <span className="c-danger" aria-hidden="true">±</span>,
  COMMENTED: <CommentIcon />,
  DISMISSED: <span className="muted" aria-hidden="true">✕</span>,
};

export function ReviewersSection({ owner, name, pr, reviews, canEdit, onChange }: {
  owner: string; name: string; pr: FullPull; reviews: FullReview[]; canEdit: boolean; onChange: (p: FullPull) => void;
}) {
  const [open, setOpen] = useState(false);
  const all = useAsync(() => (open ? papi.assignees(owner, name) : Promise.resolve([] as User[])), [owner, name, open]);
  const requested = (pr.requested_reviewers || []).map((u) => u.login);
  // latest opinion per reviewer
  const latest = new Map<string, FullReview>();
  for (const r of reviews) if (r.user && r.state !== "PENDING") latest.set(r.user.login, r);
  const people = [...new Set([...requested, ...latest.keys()])].filter((l) => l !== pr.user.login);
  const apply = async (keys: string[]) => {
    const add = keys.filter((k) => !requested.includes(k));
    const remove = requested.filter((k) => !keys.includes(k));
    if (remove.length) await papi.removeReviewers(owner, name, pr.number, remove);
    if (add.length) await papi.requestReviewers(owner, name, pr.number, add);
    onChange(await papi.pull(owner, name, pr.number));
  };
  const userOf = (login: string) =>
    pr.requested_reviewers?.find((u) => u.login === login) || latest.get(login)?.user || { login, avatar_url: "" };
  return (
    <section className="sidebar-section" data-testid="sidebar-reviewers">
      <SectionHead title="Reviewers" edit={canEdit && (
        <Popover label="Request reviewers" onOpen={() => setOpen(true)} button={<Gear />} className="btn-link btn-icon" testid="edit-reviewers">
          {(close) => {
            return (
              <SelectMenu title="Request up to 15 reviewers" multi loading={all.loading} filterPlaceholder="Type or choose a user"
                options={(all.data || []).filter((u) => u.login !== pr.user.login).map(userOption)} selected={requested}
                onApply={async (k) => { await apply(k); close(); }} />
            );
          }}
        </Popover>
      )} />
      {people.length ? (
        <ul className="pl-people">
          {people.map((l) => {
            const r = latest.get(l);
            const pending = requested.includes(l);
            return (
              <li key={l} data-reviewer={l}>
                <Avatar user={userOf(l) as User} size={20} /> <UserLink user={{ login: l }} />
                <span className="spacer" />
                {pending ? <span className="pl-dot" title="Awaiting requested review" aria-label="Awaiting requested review" /> :
                  r ? <span title={r.state.toLowerCase().replace("_", " ")} aria-label={r.state.toLowerCase().replace("_", " ")}>{REVIEW_ICON[r.state] || null}</span> : null}
              </li>
            );
          })}
        </ul>
      ) : <p className="muted small">No reviews</p>}
    </section>
  );
}

// ---------- projects (issue / PR) ----------

interface ItemNode {
  id: string;
  project: { id: string; title: string; number: number; url: string; owner: { __typename: string; login: string } };
  fieldValueByName: { name?: string | null; optionId?: string | null } | null;
}
interface ProjectLite { id: string; title: string; number: number; closed: boolean }

export function ProjectsSection({ nodeId, owner, canEdit }: { nodeId: string; owner: string; canEdit: boolean }) {
  const items = useAsync(async () => {
    const d = await gql<{ node: { projectItems: { nodes: ItemNode[] } } | null }>(
      `query($id:ID!){node(id:$id){... on Issue{projectItems(first:20,includeArchived:false){nodes{id project{id title number url owner{__typename ... on User{login} ... on Organization{login}}} fieldValueByName(name:"Status"){... on ProjectV2ItemFieldSingleSelectValue{name optionId}}}}} ... on PullRequest{projectItems(first:20,includeArchived:false){nodes{id project{id title number url owner{__typename ... on User{login} ... on Organization{login}}} fieldValueByName(name:"Status"){... on ProjectV2ItemFieldSingleSelectValue{name optionId}}}}}}}`,
      { id: nodeId },
    );
    return d.node?.projectItems.nodes || [];
  }, [nodeId]);
  const [open, setOpen] = useState(false);
  const { user } = useAuth();
  const projects = useAsync(async () => {
    if (!open || !user) return [] as ProjectLite[];
    const q = `query($o:String!,$me:String!){owner:repositoryOwner(login:$o){... on User{projectsV2(first:50){nodes{id title number closed}}} ... on Organization{projectsV2(first:50){nodes{id title number closed}}}}
      me:user(login:$me){projectsV2(first:50){nodes{id title number closed}}}}`;
    const d = await gql<{ owner: { projectsV2?: { nodes: ProjectLite[] } } | null; me: { projectsV2: { nodes: ProjectLite[] } } | null }>(q, { o: owner, me: user.login });
    const all = [...(d.owner?.projectsV2?.nodes || []), ...(d.me?.projectsV2?.nodes || [])];
    return all.filter((p, i) => !p.closed && all.findIndex((x) => x.id === p.id) === i);
  }, [open, owner, user?.login]);
  const current = (items.data || []).map((i) => i.project.id);
  const apply = async (keys: string[]) => {
    for (const pid of keys.filter((k) => !current.includes(k))) {
      await gql(`mutation($p:ID!,$c:ID!){addProjectV2ItemById(input:{projectId:$p,contentId:$c}){item{id}}}`, { p: pid, c: nodeId });
    }
    for (const it of (items.data || []).filter((i) => !keys.includes(i.project.id))) {
      await gql(`mutation($p:ID!,$i:ID!){deleteProjectV2Item(input:{projectId:$p,itemId:$i}){deletedItemId}}`, { p: it.project.id, i: it.id });
    }
    items.reload();
  };
  const href = (p: ItemNode["project"]) => `/${p.owner.__typename === "Organization" ? "orgs" : "users"}/${p.owner.login}/projects/${p.number}`;
  return (
    <section className="sidebar-section" data-testid="sidebar-projects">
      <SectionHead title="Projects" edit={canEdit && (
        <Popover label="Edit projects" onOpen={() => setOpen(true)} button={<Gear />} className="btn-link btn-icon" testid="edit-projects">
          {(close) => {
            return (
              <SelectMenu title="Add to projects" multi loading={projects.loading} filterPlaceholder="Filter projects"
                options={(projects.data || []).map((p) => ({ key: p.id, text: p.title, label: p.title }))} selected={current}
                emptyText="No projects" onApply={async (k) => { await apply(k); close(); }} />
            );
          }}
        </Popover>
      )} />
      {items.loading && !items.data ? <Spinner /> : items.error ? <ErrorBox error={items.error} /> :
        (items.data || []).length ? (
          <ul className="pl-plain">
            {(items.data || []).map((i) => (
              <li key={i.id}>
                <Link to={href(i.project)} className="pl-strong-link">{i.project.title}</Link>
                {i.fieldValueByName?.name && <div className="muted small">Status: {i.fieldValueByName.name}</div>}
              </li>
            ))}
          </ul>
        ) : <p className="muted small">None yet</p>}
    </section>
  );
}

// ---------- relationships ----------

/** Parse "#12", "12", "owner/repo#12" or an issue URL into (owner, repo, number). */
export function parseIssueRef(s: string, owner: string, name: string): { owner: string; repo: string; number: number } | null {
  const t = s.trim();
  let m = t.match(/^#?(\d+)$/);
  if (m) return { owner, repo: name, number: Number(m[1]) };
  m = t.match(/^([\w.-]+)\/([\w.-]+)#(\d+)$/);
  if (m) return { owner: m[1], repo: m[2], number: Number(m[3]) };
  m = t.match(/\/([\w.-]+)\/([\w.-]+)\/(?:issues|pull)\/(\d+)/);
  if (m) return { owner: m[1], repo: m[2], number: Number(m[3]) };
  return null;
}

function IssueLine({ i, onRemove }: { i: FullIssue; onRemove?: () => void }) {
  const repo = i.repository_url.split("/repos/")[1] || "";
  return (
    <li className="pl-issue-line">
      {i.state === "open" ? <IssueOpenIcon className="icon c-open" /> : <IssueClosedIcon className="icon c-done" />}
      <Link to={`/${repo}/issues/${i.number}`} className="pl-issue-line-title">{i.title}</Link>
      <span className="muted small">#{i.number}</span>
      {onRemove && <button type="button" className="btn-link btn-icon small" aria-label={`Remove #${i.number}`} onClick={onRemove}>✕</button>}
    </li>
  );
}

function AddIssueForm({ label, owner, name, onAdd }: { label: string; owner: string; name: string; onAdd: (i: FullIssue) => Promise<void> }) {
  const [v, setV] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <form className="pl-inline-form" onSubmit={async (e) => {
      e.preventDefault();
      const ref = parseIssueRef(v, owner, name);
      if (!ref) { setErr("Enter an issue like #12 or owner/repo#12"); return; }
      setBusy(true);
      setErr("");
      try {
        const i = await papi.issue(ref.owner, ref.repo, ref.number);
        await onAdd(i);
        setV("");
      } catch (ex) {
        setErr(errorMessage(ex));
      } finally {
        setBusy(false);
      }
    }}>
      <label className="sr-only" htmlFor={`add-${label}`}>{label}</label>
      <input id={`add-${label}`} className="input input-sm" placeholder="#issue or owner/repo#n" value={v} onChange={(e) => setV(e.target.value)} />
      <button type="submit" className="btn btn-sm" disabled={busy || !v.trim()}>Add</button>
      {err && <p className="c-danger small" role="alert">{err}</p>}
    </form>
  );
}

export function RelationshipsSection({ owner, name, issue, canEdit }: { owner: string; name: string; issue: FullIssue; canEdit: boolean }) {
  const n = issue.number;
  const deps = useAsync(async () => {
    const [by, blocking, parent] = await Promise.all([
      papi.blockedBy(owner, name, n),
      papi.blocking(owner, name, n),
      papi.parentIssue(owner, name, n).catch(() => null),
    ]);
    return { by, blocking, parent };
  }, [owner, name, n]);
  const [mode, setMode] = useState<"" | "by" | "blocking" | "parent">("");
  const d = deps.data;
  return (
    <section className="sidebar-section" data-testid="sidebar-relationships">
      <SectionHead title="Relationships" edit={canEdit && (
        <Popover label="Edit relationships" button={<Gear />} className="btn-link btn-icon" testid="edit-relationships">
          {(close) => (
            <div className="pl-menu" role="menu">
              <button type="button" role="menuitem" className="menu-item" onClick={() => { setMode("parent"); close(); }}>Add parent</button>
              <button type="button" role="menuitem" className="menu-item" onClick={() => { setMode("by"); close(); }}>Mark as blocked by</button>
              <button type="button" role="menuitem" className="menu-item" onClick={() => { setMode("blocking"); close(); }}>Mark as blocking</button>
            </div>
          )}
        </Popover>
      )} />
      {deps.loading && !d ? <Spinner /> : deps.error ? <ErrorBox error={deps.error} onRetry={deps.reload} /> : d && (
        <>
          {mode === "parent" && <AddIssueForm label="Parent issue" owner={owner} name={name} onAdd={async (p) => {
            const ref = p.repository_url.split("/repos/")[1].split("/");
            await papi.addSubIssue(ref[0], ref[1], p.number, issue.id, true);
            setMode("");
            deps.reload();
          }} />}
          {mode === "by" && <AddIssueForm label="Blocked by" owner={owner} name={name} onAdd={async (b) => {
            await papi.addBlockedBy(owner, name, n, b.id);
            setMode("");
            deps.reload();
          }} />}
          {mode === "blocking" && <AddIssueForm label="Blocking" owner={owner} name={name} onAdd={async (b) => {
            const ref = b.repository_url.split("/repos/")[1].split("/");
            await papi.addBlockedBy(ref[0], ref[1], b.number, issue.id);
            setMode("");
            deps.reload();
          }} />}
          {d.parent && (
            <div className="pl-rel" data-testid="rel-parent">
              <div className="muted small">Parent issue</div>
              <ul className="pl-plain"><IssueLine i={d.parent} onRemove={canEdit ? async () => {
                const ref = d.parent!.repository_url.split("/repos/")[1].split("/");
                await papi.removeSubIssue(ref[0], ref[1], d.parent!.number, issue.id);
                deps.reload();
              } : undefined} /></ul>
            </div>
          )}
          {d.by.length > 0 && (
            <div className="pl-rel" data-testid="rel-blocked-by">
              <div className="muted small">Blocked by</div>
              <ul className="pl-plain">{d.by.map((b) => <IssueLine key={b.id} i={b} onRemove={canEdit ? async () => {
                await papi.removeBlockedBy(owner, name, n, b.id);
                deps.reload();
              } : undefined} />)}</ul>
            </div>
          )}
          {d.blocking.length > 0 && (
            <div className="pl-rel" data-testid="rel-blocking">
              <div className="muted small">Blocking</div>
              <ul className="pl-plain">{d.blocking.map((b) => <IssueLine key={b.id} i={b} onRemove={canEdit ? async () => {
                const ref = b.repository_url.split("/repos/")[1].split("/");
                await papi.removeBlockedBy(ref[0], ref[1], b.number, issue.id);
                deps.reload();
              } : undefined} />)}</ul>
            </div>
          )}
          {!d.parent && !d.by.length && !d.blocking.length && !mode && <p className="muted small">None yet</p>}
        </>
      )}
    </section>
  );
}

/** Sub-issues box in the main column of an issue (progress, list, add existing / create). */
export function SubIssuesBox({ owner, name, issue, canEdit }: { owner: string; name: string; issue: FullIssue; canEdit: boolean }) {
  const subs = useAsync(() => papi.subIssues(owner, name, issue.number), [owner, name, issue.number]);
  const [mode, setMode] = useState<"" | "add" | "create">("");
  const [title, setTitle] = useState("");
  const [err, setErr] = useState("");
  const list = subs.data || [];
  if (!list.length && !canEdit) return null;
  const done = list.filter((i) => i.state === "closed").length;
  return (
    <div className="box pl-subissues" data-testid="sub-issues">
      <div className="box-header pl-subissues-head">
        <strong>Sub-issues</strong>
        {list.length > 0 && <span className="muted small">{done} of {list.length}</span>}
        {list.length > 0 && <Progress done={done} total={list.length} />}
        <span className="spacer" />
        {canEdit && (
          <Popover label="Add sub-issue" button={<>Add sub-issue ▾</>} testid="add-sub-issue">
            {(close) => (
              <div className="pl-menu" role="menu">
                <button type="button" role="menuitem" className="menu-item" onClick={() => { setMode("create"); close(); }}>Create sub-issue</button>
                <button type="button" role="menuitem" className="menu-item" onClick={() => { setMode("add"); close(); }}>Add existing issue</button>
              </div>
            )}
          </Popover>
        )}
      </div>
      {mode === "add" && (
        <div className="box-body">
          <AddIssueForm label="Existing issue" owner={owner} name={name} onAdd={async (i) => {
            await papi.addSubIssue(owner, name, issue.number, i.id);
            setMode("");
            subs.reload();
          }} />
        </div>
      )}
      {mode === "create" && (
        <form className="box-body pl-inline-form" onSubmit={async (e) => {
          e.preventDefault();
          setErr("");
          try {
            const c = await papi.createIssue(owner, name, { title: title.trim() });
            await papi.addSubIssue(owner, name, issue.number, c.id);
            setTitle("");
            setMode("");
            subs.reload();
          } catch (ex) {
            setErr(errorMessage(ex));
          }
        }}>
          <label className="sr-only" htmlFor="sub-title">Sub-issue title</label>
          <input id="sub-title" className="input input-sm" placeholder="Sub-issue title" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} />
          <button type="submit" className="btn btn-sm btn-primary" disabled={!title.trim()}>Create</button>
          <button type="button" className="btn btn-sm" onClick={() => setMode("")}>Cancel</button>
          {err && <p className="c-danger small" role="alert">{err}</p>}
        </form>
      )}
      {subs.error ? <div className="box-body"><ErrorBox error={subs.error} onRetry={subs.reload} /></div> :
        list.length > 0 && (
          <ul className="list">
            {list.map((i) => (
              <li key={i.id} className="list-row">
                <ul className="pl-plain grow"><IssueLine i={i} onRemove={canEdit ? async () => {
                  await papi.removeSubIssue(owner, name, issue.number, i.id);
                  subs.reload();
                } : undefined} /></ul>
              </li>
            ))}
          </ul>
        )}
      {!list.length && !mode && <div className="box-body muted small">No sub-issues yet.</div>}
    </div>
  );
}

// ---------- lock / pin / transfer / delete ----------

const LOCK_REASONS = [
  { v: "", label: "Choose a reason (optional)" },
  { v: "off-topic", label: "Off-topic" },
  { v: "too heated", label: "Too heated" },
  { v: "resolved", label: "Resolved" },
  { v: "spam", label: "Spam" },
];

export function LockControl({ owner, name, number, locked, kind, onChange }: {
  owner: string; name: string; number: number; locked: boolean; kind: "issue" | "pull request"; onChange: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [err, setErr] = useState("");
  return (
    <>
      <button type="button" className="btn-link pl-side-action" data-testid="lock-toggle" onClick={async () => {
        if (locked) {
          try { await papi.unlock(owner, name, number); onChange(); } catch (e) { setErr(errorMessage(e)); }
        } else setOpen(true);
      }}>
        <LockIcon /> {locked ? "Unlock conversation" : "Lock conversation"}
      </button>
      {err && <p className="c-danger small" role="alert">{err}</p>}
      {open && (
        <Dialog title={`Lock conversation on this ${kind}`} onClose={() => setOpen(false)}>
          <ul className="small">
            <li>Other users <strong>can't add new comments</strong> to this {kind}.</li>
            <li>You and other collaborators with access to this repository can still leave comments that others can see.</li>
            <li>You can always unlock this {kind} again in the future.</li>
          </ul>
          <div className="form-group">
            <label htmlFor="lock-reason">Reason for locking</label>
            <select id="lock-reason" className="input" value={reason} onChange={(e) => setReason(e.target.value)}>
              {LOCK_REASONS.map((r) => <option key={r.v} value={r.v}>{r.label}</option>)}
            </select>
          </div>
          {err && <p className="c-danger small" role="alert">{err}</p>}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setOpen(false)}>Cancel</button>
            <button type="button" className="btn btn-primary" onClick={async () => {
              try { await papi.lock(owner, name, number, reason || undefined); setOpen(false); onChange(); } catch (e) { setErr(errorMessage(e)); }
            }}>Lock conversation</button>
          </div>
        </Dialog>
      )}
    </>
  );
}

export function PinControl({ nodeId }: { nodeId: string }) {
  const st = useAsync(() => gql<{ node: { isPinned: boolean | null } | null }>(`query($id:ID!){node(id:$id){... on Issue{isPinned}}}`, { id: nodeId }), [nodeId]);
  const [err, setErr] = useState("");
  const pinned = !!st.data?.node?.isPinned;
  if (!st.data) return null;
  return (
    <>
      <button type="button" className="btn-link pl-side-action" data-testid="pin-toggle" data-pinned={pinned ? "true" : "false"} onClick={async () => {
        setErr("");
        try {
          await gql(pinned ? `mutation($id:ID!){unpinIssue(input:{issueId:$id}){clientMutationId}}` : `mutation($id:ID!){pinIssue(input:{issueId:$id}){issue{id}}}`, { id: nodeId });
          st.reload();
        } catch (e) { setErr(errorMessage(e)); }
      }}>
        <span aria-hidden="true">📌</span> {pinned ? "Unpin issue" : "Pin issue"}
      </button>
      {err && <p className="c-danger small" role="alert">{err}</p>}
    </>
  );
}

export function TransferControl({ issue, repo }: { issue: FullIssue; repo: Repo }) {
  const [open, setOpen] = useState(false);
  const repos = useAsync(() => (open ? api.myRepos() : Promise.resolve([] as Repo[])), [open]);
  const [target, setTarget] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const nav = useNavigate();
  const choices = (repos.data || []).filter((r) => r.id !== repo.id && r.owner.login === repo.owner.login && r.has_issues !== false);
  return (
    <>
      <button type="button" className="btn-link pl-side-action" data-testid="transfer-open" onClick={() => setOpen(true)}>
        <span aria-hidden="true">→</span> Transfer issue
      </button>
      {open && (
        <Dialog title="Transfer issue" onClose={() => setOpen(false)}>
          <p className="small muted">Move this issue to another repository owned by {repo.owner.login}.</p>
          {repos.loading ? <Spinner /> : (
            <div className="form-group">
              <label htmlFor="transfer-repo">Repository</label>
              <select id="transfer-repo" className="input" value={target} onChange={(e) => setTarget(e.target.value)}>
                <option value="">Choose a repository</option>
                {choices.map((r) => <option key={r.id} value={r.full_name}>{r.full_name}</option>)}
              </select>
            </div>
          )}
          {err && <p className="c-danger small" role="alert">{err}</p>}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setOpen(false)}>Cancel</button>
            <button type="button" className="btn btn-primary" disabled={!target || busy} onClick={async () => {
              setBusy(true);
              setErr("");
              try {
                const [o, r] = target.split("/");
                const d = await gql<{ repository: { id: string } }>(`query($o:String!,$r:String!){repository(owner:$o,name:$r){id}}`, { o, r });
                const res = await gql<{ transferIssue: { issue: { number: number; repository: { nameWithOwner: string } } } }>(
                  `mutation($i:ID!,$r:ID!){transferIssue(input:{issueId:$i,repositoryId:$r,createLabelsIfMissing:true}){issue{number repository{nameWithOwner}}}}`,
                  { i: issue.node_id, r: d.repository.id });
                setOpen(false);
                nav(`/${res.transferIssue.issue.repository.nameWithOwner}/issues/${res.transferIssue.issue.number}`);
              } catch (e) {
                setErr(errorMessage(e));
                setBusy(false);
              }
            }}>Transfer issue</button>
          </div>
        </Dialog>
      )}
    </>
  );
}

export function DeleteIssueControl({ issue, base }: { issue: FullIssue; base: string }) {
  const [open, setOpen] = useState(false);
  const [err, setErr] = useState("");
  const nav = useNavigate();
  return (
    <>
      <button type="button" className="btn-link pl-side-action c-danger" data-testid="delete-issue-open" onClick={() => setOpen(true)}>
        <span aria-hidden="true">🗑</span> Delete issue
      </button>
      {open && (
        <Dialog title="Delete issue?" onClose={() => setOpen(false)}>
          <p>This will permanently delete issue #{issue.number}. This cannot be undone.</p>
          {err && <p className="c-danger small" role="alert">{err}</p>}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setOpen(false)}>Cancel</button>
            <button type="button" className="btn btn-danger" onClick={async () => {
              try {
                await gql(`mutation($i:ID!){deleteIssue(input:{issueId:$i}){clientMutationId}}`, { i: issue.node_id });
                nav(`${base}/issues`);
              } catch (e) { setErr(errorMessage(e)); }
            }}>Delete this issue</button>
          </div>
        </Dialog>
      )}
    </>
  );
}

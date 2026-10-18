// Notifications: header bell, /notifications inbox, /watching.
import { useEffect, useMemo, useState } from "react";
import { Link, Navigate, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { errorMessage, type User } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, ErrorBox, Pagination, RelTime, Spinner } from "../../components/common";
import { CheckIcon, CommentIcon, IssueOpenIcon, PullIcon, RepoIcon, TagIcon } from "../../components/icons";
import { useAsync, useTitle } from "../../hooks";
import { sapi, type Thread } from "./api";
import { BellIcon } from "./icons";

/** Header bell with an unread dot (polls every 60 s and on navigation). */
export function NotificationBell({ user }: { user: User | null }) {
  const loc = useLocation();
  const [unread, setUnread] = useState(0);
  useEffect(() => {
    if (!user) return;
    let live = true;
    const load = () => sapi.unreadCount().then((n) => live && setUnread(n)).catch(() => {});
    load();
    const t = setInterval(load, 60000);
    const onChange = () => load();
    window.addEventListener("forge:notifications", onChange);
    return () => {
      live = false;
      clearInterval(t);
      window.removeEventListener("forge:notifications", onChange);
    };
  }, [user?.login, loc.pathname, loc.search]);
  if (!user) return null;
  const label = unread ? `You have unread notifications (${unread >= 50 ? "50+" : unread})` : "You have no unread notifications";
  return (
    <Link to="/notifications" className="header-btn notif-bell" aria-label={label} title={label} data-testid="notification-bell">
      <BellIcon />
      {unread > 0 && <span className="notif-dot" data-testid="notification-dot" />}
    </Link>
  );
}

/** Tell the bell to refresh (after marking things read). */
export const pokeBell = () => window.dispatchEvent(new Event("forge:notifications"));

/** API subject URL → SPA path. */
export function subjectPath(t: Thread): string {
  const full = t.repository.full_name;
  const u = t.subject.url || "";
  let m = u.match(/\/repos\/[^/]+\/[^/]+\/(issues|pulls)\/(\d+)/);
  if (m) return `/${full}/${m[1] === "pulls" ? "pull" : "issues"}/${m[2]}`;
  m = u.match(/\/commits\/([0-9a-f]+)/);
  if (m) return `/${full}/commit/${m[1]}`;
  if (t.subject.type === "Release") return `/${full}/releases`;
  if (t.subject.type === "RepositoryInvitation") return `/${full}/invitations`;
  return `/${full}`;
}

const REASONS: Record<string, string> = {
  assign: "Assigned",
  author: "Author",
  comment: "Comment",
  invitation: "Invitation",
  manual: "Subscribed",
  mention: "Mention",
  review_requested: "Review requested",
  state_change: "State change",
  subscribed: "Watching",
  team_mention: "Team mention",
  ci_activity: "CI activity",
  security_alert: "Security alert",
};

function TypeIcon({ type }: { type: string }) {
  if (type === "PullRequest") return <PullIcon />;
  if (type === "Issue") return <IssueOpenIcon />;
  if (type === "Release") return <TagIcon />;
  if (type === "Commit") return <CommentIcon />;
  return <RepoIcon />;
}

function ThreadRow({ t, onChange }: { t: Thread; onChange: (t: Thread | null) => void }) {
  const nav = useNavigate();
  const [busy, setBusy] = useState(false);
  const run = async (fn: () => Promise<unknown>, after: Thread | null) => {
    setBusy(true);
    try {
      await fn();
      onChange(after);
      pokeBell();
    } catch (e) {
      alert(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const open = async (e: React.MouseEvent) => {
    e.preventDefault();
    if (t.unread) {
      try {
        await sapi.markThreadRead(t.id);
        pokeBell();
      } catch {
        /* still navigate */
      }
    }
    nav(subjectPath(t));
  };
  return (
    <li className={"list-row notif-row" + (t.unread ? " unread" : "")} data-testid="notification-row" data-unread={t.unread ? "1" : "0"}>
      <span className="notif-unread-dot" aria-label={t.unread ? "Unread" : undefined} />
      <span className="notif-type muted"><TypeIcon type={t.subject.type} /></span>
      <div className="notif-main">
        <div className="muted small">
          <Link to={`/${t.repository.full_name}`}>{t.repository.full_name}</Link>
        </div>
        <a href={subjectPath(t)} onClick={open} className="notif-title" data-testid="notification-title">{t.subject.title}</a>
        <div className="muted small">
          <span className="badge" data-testid="notification-reason">{REASONS[t.reason] || t.reason}</span>{" "}
          <RelTime iso={t.updated_at} />
        </div>
      </div>
      <div className="notif-actions">
        {t.unread ? (
          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => run(() => sapi.markThreadRead(t.id), { ...t, unread: false })}
            data-testid="notification-mark-read" title="Mark as read">Mark as read</button>
        ) : null}
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => run(() => sapi.markThreadDone(t.id), null)}
          data-testid="notification-done" title="Done"><CheckIcon /> Done</button>
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => run(async () => { await sapi.unsubscribeThread(t.id); await sapi.markThreadDone(t.id); }, null)}
          data-testid="notification-unsubscribe" title="Unsubscribe">Unsubscribe</button>
      </div>
    </li>
  );
}

export function NotificationsPage() {
  const { user, loading } = useAuth();
  const [sp, setSp] = useSearchParams();
  useTitle("Notifications");
  const filter = sp.get("filter") || "unread"; // unread | all | participating
  const repo = sp.get("repo") || "";
  const reason = sp.get("reason") || "";
  const page = Number(sp.get("page") || 1);
  const q = { all: filter !== "unread", participating: filter === "participating", page };
  const st = useAsync(
    () => (repo ? sapi.repoNotifications(repo.split("/")[0], repo.split("/")[1], q) : sapi.notifications(q)),
    [user?.login, filter, repo, page],
  );
  // Repositories that have notifications (for the sidebar), from the unfiltered "all" list.
  const repos = useAsync(() => (user ? sapi.notifications({ all: true }) : Promise.resolve(null)), [user?.login, st.data]);
  const repoNames = useMemo(() => {
    const m = new Map<string, number>();
    for (const t of repos.data?.items || []) if (t.unread) m.set(t.repository.full_name, (m.get(t.repository.full_name) || 0) + 1);
    for (const t of repos.data?.items || []) if (!m.has(t.repository.full_name)) m.set(t.repository.full_name, 0);
    return [...m.entries()];
  }, [repos.data]);
  const [busy, setBusy] = useState(false);
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Fnotifications" replace />;
  const set = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v);
    else n.delete(k);
    n.delete("page");
    setSp(n);
  };
  const markAll = async () => {
    setBusy(true);
    try {
      if (repo) await sapi.markRepoRead(repo.split("/")[0], repo.split("/")[1]);
      else await sapi.markAllRead();
      st.reload();
      pokeBell();
    } catch (e) {
      alert(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const items = (st.data?.items || []).filter((t) => !reason || t.reason === reason);
  return (
    <div className="container notif-layout">
      <aside className="notif-side">
        <nav className="menu-list" aria-label="Notification filters">
          <Link to="/notifications" className={"menu-item" + (filter === "unread" && !repo && !reason ? " selected" : "")} data-testid="notif-filter-inbox">Inbox</Link>
          <Link to="/notifications?filter=all" className={"menu-item" + (filter === "all" && !repo && !reason ? " selected" : "")} data-testid="notif-filter-all">All</Link>
          <Link to="/notifications?filter=participating" className={"menu-item" + (filter === "participating" ? " selected" : "")} data-testid="notif-filter-participating">Participating</Link>
        </nav>
        <h2 className="h6 notif-side-head">Filters</h2>
        <nav className="menu-list" aria-label="Reasons">
          {[["assign", "Assigned"], ["mention", "Mentioned"], ["review_requested", "Review requested"], ["author", "Authored"], ["comment", "Commented"], ["subscribed", "Watching"]].map(([k, l]) => (
            <button key={k} type="button" className={"menu-item" + (reason === k ? " selected" : "")} onClick={() => set("reason", reason === k ? "" : k)}
              data-testid={`notif-reason-${k}`}>{l}</button>
          ))}
        </nav>
        <h2 className="h6 notif-side-head">Repositories</h2>
        <nav className="menu-list" aria-label="Repositories">
          {repoNames.length === 0 && <span className="muted small">No repositories</span>}
          {repoNames.map(([name, n]) => (
            <button key={name} type="button" className={"menu-item" + (repo === name ? " selected" : "")} onClick={() => set("repo", repo === name ? "" : name)}
              data-testid="notif-repo-filter">
              <span className="notif-repo-name">{name}</span> {n > 0 && <span className="counter">{n}</span>}
            </button>
          ))}
        </nav>
        <p className="small"><Link to="/watching">Manage watched repositories</Link></p>
      </aside>
      <section className="notif-main-col">
        <div className="toolbar">
          <h1 className="h4">Notifications</h1>
          <span className="spacer" />
          <div className="btn-group" role="group" aria-label="Read state">
            <button type="button" className={"btn btn-sm" + (filter === "all" ? " selected" : "")} onClick={() => set("filter", "all")} aria-pressed={filter === "all"}>All</button>
            <button type="button" className={"btn btn-sm" + (filter === "unread" ? " selected" : "")} onClick={() => set("filter", "")} aria-pressed={filter === "unread"}
              data-testid="notif-unread-toggle">Unread</button>
          </div>
          <button type="button" className="btn btn-sm" disabled={busy} onClick={markAll} data-testid="notif-mark-all">Mark all as read</button>
        </div>
        {(repo || reason) && (
          <p className="muted small">
            Filtered by {repo && <code>repo:{repo}</code>} {reason && <code>reason:{reason}</code>}{" "}
            <Link to={`/notifications${filter !== "unread" ? `?filter=${filter}` : ""}`}>Clear filters</Link>
          </p>
        )}
        <Async state={st}>
          {() =>
            items.length === 0 ? (
              <Blankslate title={filter === "unread" ? "All caught up!" : "No notifications"}>
                {filter === "unread" ? "Take a break, write some code, do what you do best." : "Notifications about activity you're watching or participating in show up here."}
              </Blankslate>
            ) : (
              <ul className="box list" data-testid="notification-list">
                {items.map((t) => (
                  <ThreadRow key={t.id} t={t} onChange={(n) => {
                    if (!st.data) return;
                    const keep = n && (filter !== "unread" || n.unread);
                    st.setData({ ...st.data, items: st.data.items.flatMap((x) => (x.id === t.id ? (keep ? [n!] : []) : [x])) });
                  }} />
                ))}
              </ul>
            )
          }
        </Async>
        {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last} onPage={(n) => { const x = new URLSearchParams(sp); x.set("page", String(n)); setSp(x); }} />}
      </section>
    </div>
  );
}

/** /watching: repositories the user watches. */
export function WatchingPage() {
  const { user, loading } = useAuth();
  const [page, setPage] = useState(1);
  useTitle("Watching");
  const st = useAsync(() => (user ? sapi.watched(page) : Promise.resolve(null)), [user?.login, page]);
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Fwatching" replace />;
  return (
    <div className="container narrow">
      <div className="toolbar">
        <h1 className="h3">Watched repositories</h1>
        <span className="spacer" />
        <Link to="/notifications" className="btn btn-sm">Notifications</Link>
      </div>
      <p className="muted">Notifications for all activity in these repositories are sent to your inbox.</p>
      {st.error ? <ErrorBox error={st.error} onRetry={st.reload} /> : !st.data ? <Spinner /> : st.data.items.length === 0 ? (
        <Blankslate title="You aren't watching any repositories">Use the Watch button on a repository to get notified about its activity.</Blankslate>
      ) : (
        <ul className="box list" data-testid="watching-list">
          {st.data.items.map((r) => (
            <li key={r.id} className="list-row watching-row">
              <Avatar user={r.owner} size={20} />
              <Link to={`/${r.full_name}`} className="watching-name">{r.full_name}</Link>
              <span className="spacer" />
              <button type="button" className="btn btn-sm" data-testid="unwatch-button" onClick={async () => {
                try {
                  await sapi.unwatch(r.owner.login, r.name);
                  st.reload();
                } catch (e) {
                  alert(errorMessage(e));
                }
              }}>Unwatch</button>
            </li>
          ))}
        </ul>
      )}
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last} onPage={setPage} />}
    </div>
  );
}

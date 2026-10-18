// Repository watching (header button), stargazers/watchers pages, issue/PR subscriptions.
import { useEffect, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { errorMessage, type Issue, type Page, type Pull, type User } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, Pagination } from "../../components/common";
import { ChevronDownIcon, StarIcon } from "../../components/icons";
import { useRepo, type RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { sapi, type WatchState } from "./api";
import { BellIcon, EyeIcon } from "./icons";

const OPTIONS: { state: WatchState["state"]; title: string; desc: string }[] = [
  { state: "not_watching", title: "Participating and @mentions", desc: "Only receive notifications from this repository when participating or @mentioned." },
  { state: "watching", title: "All Activity", desc: "Notified of all notifications on this repository." },
  { state: "ignoring", title: "Ignore", desc: "Never be notified." },
  { state: "custom", title: "Custom", desc: "Select events you want to be notified of in addition to participating and @mentions." },
];
const EVENTS: [string, string][] = [["issues", "Issues"], ["pulls", "Pull requests"], ["releases", "Releases"]];

function useClickOutside(open: boolean, close: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const h = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof KeyboardEvent ? e.key === "Escape" : !ref.current?.contains(e.target as Node)) close();
    };
    document.addEventListener("mousedown", h);
    document.addEventListener("keydown", h);
    return () => {
      document.removeEventListener("mousedown", h);
      document.removeEventListener("keydown", h);
    };
  }, [open, close]);
  return ref;
}

export function WatchButton({ ctx }: { ctx: RepoCtx }) {
  const { user } = useAuth();
  const nav = useNavigate();
  const loc = useLocation();
  const st = useAsync(() => (user ? sapi.watchState(ctx.owner, ctx.name) : Promise.resolve(null)), [user?.login, ctx.owner, ctx.name]);
  const [open, setOpen] = useState(false);
  const [custom, setCustom] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const ref = useClickOutside(open, () => { setOpen(false); setCustom(null); });
  const count = st.data?.subscribers_count ?? (ctx.repo as { subscribers_count?: number }).subscribers_count ?? 0;
  const state = st.data?.state || "not_watching";
  const set = async (s: WatchState["state"], events: string[] = []) => {
    setBusy(true);
    try {
      st.setData(await sapi.setWatch(ctx.owner, ctx.name, s, events));
      setOpen(false);
      setCustom(null);
    } catch (e) {
      alert(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const label = state === "watching" ? "Unwatch" : state === "ignoring" ? "Stop ignoring" : state === "custom" ? "Custom" : "Watch";
  return (
    <div className="btn-group menu-wrap" ref={ref}>
      <button type="button" className="btn btn-sm" aria-haspopup="menu" aria-expanded={open} data-testid="watch-button" disabled={busy}
        onClick={() => (user ? setOpen(!open) : nav(`/login?return_to=${encodeURIComponent(loc.pathname)}`))}>
        {state === "ignoring" ? <BellIcon /> : <EyeIcon />} {label} <ChevronDownIcon size={12} />
      </button>
      <Link to={`${ctx.base}/watchers`} className="btn btn-sm btn-counter" title="Watchers" data-testid="watch-count">{count}</Link>
      {open && (
        <div className="menu watch-menu" role="menu" data-testid="watch-menu">
          <div className="menu-header">Notifications</div>
          {custom === null ? OPTIONS.map((o) => (
            <button key={o.state} type="button" role="menuitemradio" aria-checked={state === o.state} className="menu-item watch-option"
              data-testid={`watch-option-${o.state}`}
              onClick={() => (o.state === "custom" ? setCustom(st.data?.state === "custom" ? st.data.events : []) : set(o.state))}>
              <span className="watch-check">{state === o.state ? "✓" : ""}</span>
              <span><strong>{o.title}</strong><br /><span className="muted small">{o.desc}</span></span>
            </button>
          )) : (
            <div className="watch-custom" data-testid="watch-custom">
              <p className="small muted">Select events you want to be notified of in addition to participating and @mentions.</p>
              {EVENTS.map(([k, l]) => (
                <label key={k} className="check-row">
                  <input type="checkbox" checked={custom.includes(k)} data-testid={`watch-custom-${k}`}
                    onChange={(e) => setCustom(e.target.checked ? [...custom, k] : custom.filter((x) => x !== k))} /> {l}
                </label>
              ))}
              <div className="toolbar">
                <button type="button" className="btn btn-sm" onClick={() => setCustom(null)}>Cancel</button>
                <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => set("custom", custom)} data-testid="watch-custom-apply">Apply</button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function UserGrid({ state, page, setPage, empty }: {
  state: { data: Page<User[]> | undefined; error: unknown; loading: boolean; reload: () => void };
  page: number;
  setPage: (n: number) => void;
  empty: string;
}) {
  return (
    <>
      <Async state={state}>
        {(p) => p.items.length === 0 ? <Blankslate title={empty} /> : (
          <ul className="user-grid" data-testid="user-grid">
            {p.items.map((u) => (
              <li key={u.id} className="user-grid-item">
                <Avatar user={u} size={48} />
                <Link to={`/${u.login}`} className="user-grid-login">{u.login}</Link>
              </li>
            ))}
          </ul>
        )}
      </Async>
      {state.data && <Pagination page={page} next={state.data.next} prev={state.data.prev} last={state.data.last} onPage={setPage} />}
    </>
  );
}

export function StargazersPage() {
  const ctx = useRepo();
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  useTitle(`Stargazers · ${ctx.owner}/${ctx.name}`);
  const st = useAsync(() => sapi.stargazers(ctx.owner, ctx.name, page), [ctx.owner, ctx.name, page, ctx.repo.stargazers_count]);
  return (
    <div className="container social-page">
      <h1 className="h3"><StarIcon /> Stargazers</h1>
      <UserGrid state={st} page={page} setPage={(n) => setSp({ page: String(n) })} empty="Be the first to star this repository" />
    </div>
  );
}

export function WatchersPage() {
  const ctx = useRepo();
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  useTitle(`Watchers · ${ctx.owner}/${ctx.name}`);
  const st = useAsync(() => sapi.watchers(ctx.owner, ctx.name, page), [ctx.owner, ctx.name, page]);
  return (
    <div className="container social-page">
      <h1 className="h3"><EyeIcon /> Watchers</h1>
      <UserGrid state={st} page={page} setPage={(n) => setSp({ page: String(n) })} empty="No one's watching this repository yet" />
    </div>
  );
}

function SubscribeBox({ ctx, number, kind }: { ctx: RepoCtx; number: number; kind: "issue" | "pull request" }) {
  const { user } = useAuth();
  const st = useAsync(() => (user ? sapi.issueSub(ctx.owner, ctx.name, number) : Promise.resolve(null)), [user?.login, ctx.owner, ctx.name, number]);
  const [busy, setBusy] = useState(false);
  if (!user || !st.data) return null;
  const on = st.data.subscribed;
  const why: Record<string, string> = {
    author: `You're receiving notifications because you authored this ${kind}.`,
    assign: `You're receiving notifications because you were assigned.`,
    comment: `You're receiving notifications because you commented.`,
    subscribed: "You're receiving notifications because you're watching this repository.",
    manual: "You're receiving notifications because you're subscribed to this thread.",
  };
  return (
    <section className="sidebar-section" data-testid="subscription-box">
      <h3 className="h6">Notifications</h3>
      <button type="button" className="btn btn-sm btn-block" disabled={busy} data-testid="subscribe-button"
        onClick={async () => {
          setBusy(true);
          try {
            st.setData(await sapi.setIssueSub(ctx.owner, ctx.name, number, !on));
          } catch (e) {
            alert(errorMessage(e));
          } finally {
            setBusy(false);
          }
        }}>
        <BellIcon /> {on ? "Unsubscribe" : "Subscribe"}
      </button>
      <p className="muted small" data-testid="subscription-reason">
        {on ? why[st.data.reason] || "You're receiving notifications." : `You're not receiving notifications from this ${kind === "issue" ? "thread" : kind}.`}
      </p>
    </section>
  );
}

export const IssueSubscribe = ({ ctx, issue }: { ctx: RepoCtx; issue: Issue }) => <SubscribeBox ctx={ctx} number={issue.number} kind="issue" />;
export const PullSubscribe = ({ ctx, pull }: { ctx: RepoCtx; pull: Pull }) => <SubscribeBox ctx={ctx} number={pull.number} kind="pull request" />;

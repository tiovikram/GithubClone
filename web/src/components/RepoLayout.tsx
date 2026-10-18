import { useState } from "react";
import type { RepoTab } from "../areas/types";
import { Link, NavLink, Navigate, Outlet, useLocation, useNavigate, useOutletContext, useParams } from "react-router-dom";
import { api, errorMessage, type Repo } from "../api";
import { useAuth } from "../auth";
import { useAsync } from "../hooks";
import { repoTabs, Slot } from "../areas";
import { Async } from "./common";
import { CodeIcon, ForkIcon, GearIcon, IssueOpenIcon, LockIcon, PullIcon, RepoIcon, StarIcon, TagIcon } from "./icons";

function StarButton({ repo, onChange }: { repo: Repo; onChange: (count: number) => void }) {
  const { user } = useAuth();
  const nav = useNavigate();
  const loc = useLocation();
  const owner = repo.owner.login;
  const st = useAsync(() => (user ? api.isStarred(owner, repo.name) : Promise.resolve(false)), [user?.login, owner, repo.name]);
  const [busy, setBusy] = useState(false);
  const starred = !!st.data;
  const toggle = async () => {
    if (!user) {
      nav(`/login?return_to=${encodeURIComponent(loc.pathname)}`);
      return;
    }
    setBusy(true);
    try {
      if (starred) await api.unstar(owner, repo.name);
      else await api.star(owner, repo.name);
      st.setData(!starred);
      onChange(Math.max(0, repo.stargazers_count + (starred ? -1 : 1)));
    } catch (e) {
      alert(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="btn-group">
      <button type="button" className="btn btn-sm" onClick={toggle} disabled={busy || st.loading}
        aria-pressed={starred} data-testid="star-button">
        <StarIcon className={starred ? "icon c-star" : "icon"} /> {starred ? "Starred" : "Star"}
      </button>
      <Link to={`/${owner}/${repo.name}/stargazers`} className="btn btn-sm btn-counter" title="Stars" data-testid="star-count">{repo.stargazers_count}</Link>
    </div>
  );
}

function ForkButton({ repo }: { repo: Repo }) {
  const { user } = useAuth();
  const nav = useNavigate();
  const loc = useLocation();
  const [busy, setBusy] = useState(false);
  const own = user?.login === repo.owner.login;
  const fork = async () => {
    if (!user) {
      nav(`/login?return_to=${encodeURIComponent(loc.pathname)}`);
      return;
    }
    setBusy(true);
    try {
      const f = await api.forkRepo(repo.owner.login, repo.name);
      nav(`/${f.full_name}`);
    } catch (e) {
      alert(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="btn-group">
      <button type="button" className="btn btn-sm" onClick={fork} disabled={busy || own}
        title={own ? "You cannot fork your own repository" : "Fork this repository"} data-testid="fork-button">
        <ForkIcon /> {busy ? "Forking…" : "Fork"}
      </button>
      <span className="btn btn-sm btn-counter" title="Forks" data-testid="fork-count">{repo.forks_count}</span>
    </div>
  );
}

function AreaTab({ t, base }: { t: RepoTab; base: string }) {
  const loc = useLocation();
  const rest = loc.pathname.slice(base.length);
  const on = rest === `/${t.path}` || rest.startsWith(`/${t.path}/`) || (t.alsoActive || []).some((p) => rest.startsWith(p));
  const Icon = t.icon;
  return (
    <NavLink to={`${base}/${t.path}`} className={() => "tab" + (on ? " selected" : "")} data-testid={`repo-tab-${t.key}`}>
      {Icon && <Icon />} {t.label}
    </NavLink>
  );
}

export interface RepoCtx {
  repo: Repo;
  owner: string;
  name: string;
  /** "owner/name" as used in SPA URLs. */
  base: string;
  canPush: boolean;
  isAdmin: boolean;
  reloadRepo: () => void;
  setRepo: (r: Repo) => void;
}

export const useRepo = () => useOutletContext<RepoCtx>();

export function RepoLayout() {
  const { owner = "", repo: name = "" } = useParams();
  const loc = useLocation();
  const state = useAsync(() => api.repo(owner, name), [owner, name]);
  if (name.endsWith(".git")) return <Navigate to={`/${owner}/${name.slice(0, -4)}`} replace />;
  return (
    <Async state={state}>
      {(repo) => {
        const base = `/${repo.owner.login}/${repo.name}`;
        const ctx: RepoCtx = {
          repo,
          owner: repo.owner.login,
          name: repo.name,
          base,
          canPush: !!repo.permissions?.push,
          isAdmin: !!repo.permissions?.admin,
          reloadRepo: state.reload,
          setRepo: (r) => state.setData(r),
        };
        const tab = ({ isActive }: { isActive: boolean }) => "tab" + (isActive ? " selected" : "");
        return (
          <div className="repo">
            <div className="repo-head">
              <div className="container repo-title-row">
                <h1 className="repo-title">
                  {repo.private ? <LockIcon /> : <RepoIcon />}
                  <Link to={`/${repo.owner.login}`}>{repo.owner.login}</Link>
                  <span className="sep">/</span>
                  <Link to={base}><strong>{repo.name}</strong></Link>
                  <span className="badge">{repo.private ? "Private" : "Public"}</span>
                </h1>
                {repo.fork && repo.parent && (
                  <div className="muted small fork-note">
                    forked from <Link to={`/${repo.parent.full_name}`}>{repo.parent.full_name}</Link>
                  </div>
                )}
                <div className="repo-actions">
                  <Slot name="repo.header.actions" ctx={ctx} />
                  <ForkButton repo={repo} />
                  <StarButton repo={repo} onChange={(n) => state.setData({ ...repo, stargazers_count: n })} />
                </div>
              </div>
              <nav className="container tabs" aria-label="Repository">
                <NavLink to={base} end className={({ isActive }) => {
                  const p = loc.pathname.slice(base.length);
                  return "tab" + (isActive || /^\/(tree|blob|commits?)\//.test(p) || p.startsWith("/commits") ? " selected" : "");
                }}>
                  <CodeIcon /> Code
                </NavLink>
                <NavLink to={`${base}/issues`} className={tab}>
                  <IssueOpenIcon /> Issues
                </NavLink>
                <NavLink to={`${base}/pulls`} className={({ isActive }) =>
                  "tab" + (isActive || /^\/(pull|compare)/.test(loc.pathname.slice(base.length)) ? " selected" : "")}>
                  <PullIcon /> Pull requests
                </NavLink>
                {repoTabs.filter((t) => t.order < 80 && (!t.show || t.show(ctx))).map((t) => <AreaTab key={t.key} t={t} base={base} />)}
                <NavLink to={`${base}/releases`} className={tab}>
                  <TagIcon /> Releases
                </NavLink>
                {repoTabs.filter((t) => t.order >= 80 && (!t.show || t.show(ctx))).map((t) => <AreaTab key={t.key} t={t} base={base} />)}
                {ctx.isAdmin && (
                  <NavLink to={`${base}/settings`} className={tab}>
                    <GearIcon /> Settings
                  </NavLink>
                )}
              </nav>
            </div>
            <div className="container repo-body">
              <Outlet context={ctx} />
            </div>
          </div>
        );
      }}
    </Async>
  );
}

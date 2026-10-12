import { Link, NavLink, Navigate, Outlet, useLocation, useOutletContext, useParams } from "react-router-dom";
import { api, type Repo } from "../api";
import { useAsync } from "../hooks";
import { Async } from "./common";
import { CodeIcon, ForkIcon, GearIcon, IssueOpenIcon, LockIcon, PullIcon, RepoIcon, StarIcon } from "./icons";

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
                <div className="repo-counters muted">
                  <span title="Stars"><StarIcon /> {repo.stargazers_count}</span>
                  <span title="Forks"><ForkIcon /> {repo.forks_count}</span>
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

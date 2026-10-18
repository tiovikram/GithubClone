// Shared pieces of the organization pages: loading an org with the viewer's membership, the org
// header with its tabs, small form helpers.
import { useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { isNotFound } from "../../api";
import { useAuth } from "../../auth";
import { Avatar, ErrorBox, NotFoundView, Spinner } from "../../components/common";
import { BookIcon, GearIcon, RepoIcon } from "../../components/icons";
import { useAsync } from "../../hooks";
import { orgApi, type Membership, type Org } from "./api";

export interface OrgState {
  org: Org;
  /** The viewer's membership (active or pending), null when none / signed out. */
  membership: Membership | null;
  isMember: boolean;
  isOwner: boolean;
  reload: () => void;
}

/** Loads `/orgs/:org` and the viewer's membership. */
export function useOrg(login: string) {
  const { user } = useAuth();
  return useAsync(async () => {
    const org = await orgApi.org(login);
    let membership: Membership | null = null;
    if (user) {
      try {
        membership = await orgApi.myMembership(org.login);
      } catch (e) {
        if (!isNotFound(e)) throw e;
      }
    }
    const active = membership?.state === "active";
    return {
      org,
      membership,
      isMember: active || !!user?.site_admin,
      isOwner: (active && membership?.role === "admin") || !!user?.site_admin,
    };
  }, [login, user?.login]);
}

/** Renders children with the loaded org (404 view for unknown logins / users). */
export function WithOrg({ login, children }: { login: string; children: (s: OrgState) => ReactNode }) {
  const st = useOrg(login);
  if (st.error) return isNotFound(st.error) ? <NotFoundView /> : <ErrorBox error={st.error} onRetry={st.reload} />;
  if (!st.data) return <Spinner />;
  if (st.data.org.type && st.data.org.type !== "Organization") return <NotFoundView />;
  return <>{children({ ...st.data, reload: st.reload })}</>;
}

export type OrgTabKey = "overview" | "repositories" | "projects" | "packages" | "people" | "teams" | "settings";

const PeopleIcon = (p: { size?: number }) => (
  <svg width={p.size || 16} height={p.size || 16} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" className="icon">
    <path d="M2 5.5a3.5 3.5 0 1 1 5.898 2.549 5.508 5.508 0 0 1 3.034 4.084.75.75 0 1 1-1.482.235 4 4 0 0 0-7.9 0 .75.75 0 0 1-1.482-.236A5.507 5.507 0 0 1 3.102 8.05 3.493 3.493 0 0 1 2 5.5ZM11 4a3.001 3.001 0 0 1 2.22 5.018 5.01 5.01 0 0 1 2.56 3.012.749.749 0 0 1-.885.954.752.752 0 0 1-.549-.514 3.507 3.507 0 0 0-2.522-2.372.75.75 0 0 1-.574-.73v-.352a.75.75 0 0 1 .416-.672A1.5 1.5 0 0 0 11 5.5.75.75 0 0 1 11 4Zm-5.5-.5a2 2 0 1 0-.001 3.999A2 2 0 0 0 5.5 3.5Z" />
  </svg>
);
const ProjectIcon = (p: { size?: number }) => (
  <svg width={p.size || 16} height={p.size || 16} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" className="icon">
    <path d="M1.75 0h12.5C15.216 0 16 .784 16 1.75v12.5A1.75 1.75 0 0 1 14.25 16H1.75A1.75 1.75 0 0 1 0 14.25V1.75C0 .784.784 0 1.75 0ZM1.5 1.75v12.5c0 .138.112.25.25.25h12.5a.25.25 0 0 0 .25-.25V1.75a.25.25 0 0 0-.25-.25H1.75a.25.25 0 0 0-.25.25ZM11.75 3a.75.75 0 0 1 .75.75v7.5a.75.75 0 0 1-1.5 0v-7.5a.75.75 0 0 1 .75-.75Zm-8.25.75a.75.75 0 0 1 1.5 0v5.5a.75.75 0 0 1-1.5 0ZM8 3a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 8 3Z" />
  </svg>
);
const PackageIcon = (p: { size?: number }) => (
  <svg width={p.size || 16} height={p.size || 16} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" className="icon">
    <path d="m8.878.392 5.25 3.045c.54.314.872.89.872 1.514v6.098a1.75 1.75 0 0 1-.872 1.514l-5.25 3.045a1.75 1.75 0 0 1-1.756 0l-5.25-3.045A1.75 1.75 0 0 1 1 11.049V4.951c0-.624.332-1.201.872-1.514L7.122.392a1.75 1.75 0 0 1 1.756 0ZM7.875 1.69l-4.63 2.685L8 7.133l4.755-2.758-4.63-2.685a.248.248 0 0 0-.25 0ZM2.5 5.677v5.372c0 .09.047.171.125.216l4.625 2.683V8.432Zm6.25 8.271 4.625-2.683a.25.25 0 0 0 .125-.216V5.677L8.75 8.432Z" />
  </svg>
);
export const TeamIcon = PeopleIcon;

/** The org header (avatar, name, details) and the org tab bar. */
export function OrgHeader({ st, active }: { st: OrgState; active: OrgTabKey }) {
  const { org } = st;
  const base = `/orgs/${org.login}`;
  const tab = (key: OrgTabKey, to: string, label: string, icon: ReactNode) => (
    <Link key={key} to={to} className={"tabnav-tab" + (active === key ? " selected" : "")} data-testid={`org-tab-${key}`}
      aria-current={active === key ? "page" : undefined}>
      {icon} {label}
    </Link>
  );
  return (
    <div className="org-header">
      <div className="container org-header-inner">
        <div className="org-title">
          <Avatar user={org} size={72} />
          <div className="org-title-text">
            <h1 className="h2 org-name">{org.name || org.login}</h1>
            {org.description && <p className="muted org-desc">{org.description}</p>}
            <ul className="org-details muted small">
              {org.location && <li>{org.location}</li>}
              {org.blog && <li><a href={org.blog.startsWith("http") ? org.blog : `https://${org.blog}`} rel="nofollow noopener">{org.blog}</a></li>}
              {org.email && <li><a href={`mailto:${org.email}`}>{org.email}</a></li>}
              {st.membership?.state === "pending" && (
                <li><Link to={`${base}/invitation`} className="badge badge-warn">You've been invited</Link></li>
              )}
            </ul>
          </div>
        </div>
        <nav className="tabnav org-tabs" aria-label="Organization">
          {tab("overview", `/${org.login}`, "Overview", <BookIcon />)}
          {tab("repositories", `${base}/repositories`, "Repositories", <RepoIcon />)}
          {tab("projects", `${base}/projects`, "Projects", <ProjectIcon />)}
          {tab("packages", `${base}/packages`, "Packages", <PackageIcon />)}
          {(st.isMember || active === "people") && tab("people", `${base}/people`, "People", <PeopleIcon />)}
          {st.isMember && tab("teams", `${base}/teams`, "Teams", <PeopleIcon />)}
          {st.isOwner && tab("settings", `/organizations/${org.login}/settings`, "Settings", <GearIcon />)}
        </nav>
      </div>
    </div>
  );
}

/** A full org page: header + body in a container. */
export function OrgPage({ login, active, children }: { login: string; active: OrgTabKey; children: (s: OrgState) => ReactNode }) {
  return (
    <WithOrg login={login}>
      {(st) => (
        <>
          <OrgHeader st={st} active={active} />
          <div className="container org-body">{children(st)}</div>
        </>
      )}
    </WithOrg>
  );
}

/** Runs an async action, tracking busy/error state. */
export function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    try {
      await fn();
      return true;
    } catch (e) {
      setError(e);
      return false;
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, setError, run };
}

export const roleLabel = (r: string) => (r === "admin" ? "Owner" : r === "member" || r === "direct_member" ? "Member" : r.replace(/_/g, " "));

export const permLabel: Record<string, string> = {
  none: "No permission",
  read: "Read",
  pull: "Read",
  triage: "Triage",
  write: "Write",
  push: "Write",
  maintain: "Maintain",
  admin: "Admin",
};

import { NavLink, Route } from "react-router-dom";
import type { RepoCtx } from "../../components/RepoLayout";
import { NotFoundView } from "../../components/common";
import { useTitle } from "../../hooks";
import type { AdminCtx, Area, OrgCtx } from "../types";
import { ENTERPRISE, actionsApi as A, type Scope } from "./api";
import { CommitChecks, DeploymentsPage, DeploymentsSidebar, PullChecksBox, PullChecksPage } from "./checks";
import { EnvironmentsSection } from "./environments";
import { PolicyPage } from "./policy";
import { ActionsRoot } from "./runs";
import { NewRunner, RunnerGroupForm, RunnerGroupsList, RunnersList } from "./runners";
import { InheritedList, SecretForm, SecretsList, SecretsTabs, VariableForm, VariablesList } from "./secrets";
import { useSubRoute } from "./ui";
import "./styles.css";

const PlayIcon = ({ size = 16, className }: { size?: number; className?: string }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5} aria-hidden="true" className={className || "icon"}>
    <circle cx="8" cy="8" r="6.25" /><path d="M6.5 5.5v5l4-2.5z" fill="currentColor" stroke="none" />
  </svg>
);

/** General | Runners | Runner groups tabs of an Actions settings section. */
function ActionsTabs({ root, groups }: { root: string; groups?: boolean }) {
  const cls = ({ isActive }: { isActive: boolean }) => "tabnav-tab" + (isActive ? " selected" : "");
  return (
    <nav className="tabnav" aria-label="Actions settings">
      <NavLink className={cls} to={`${root}/actions`} end>General</NavLink>
      <NavLink className={cls} to={`${root}/actions/runners`}>Runners</NavLink>
      {groups && <NavLink className={cls} to={`${root}/actions/runner-groups`}>Runner groups</NavLink>}
    </nav>
  );
}

/**
 * The pages under `{settings root}/actions/...` for any scope. Sub-pages are matched against
 * the absolute `root` (not nested <Routes>), so they work however the settings shell is mounted.
 */
function ActionsSettings({ scope, root, ownerName, groups }: { scope: Scope; root: string; ownerName: string; groups?: boolean }) {
  const sub = useSubRoute(root, ["/actions", "/actions/runners", "/actions/runners/new", ...(groups ? ["/actions/runner-groups", "/actions/runner-groups/:gid"] : [])]);
  const runners = `${root}/actions/runners`;
  const rgroups = `${root}/actions/runner-groups`;
  let page: React.ReactNode = <NotFoundView />;
  switch (sub?.pattern) {
    case "/actions": page = <PolicyPage scope={scope} ownerName={ownerName} />; break;
    case "/actions/runners": page = <RunnersList scope={scope} root={runners} groups={groups} />; break;
    case "/actions/runners/new": page = <NewRunner scope={scope} root={runners} />; break;
    case "/actions/runner-groups": page = <RunnerGroupsList scope={scope} root={rgroups} />; break;
    case "/actions/runner-groups/:gid": page = <RunnerGroupForm key={sub.params.gid} scope={scope} root={rgroups} gid={sub.params.gid} />; break;
  }
  return (
    <>
      <ActionsTabs root={root} groups={groups} />
      {page}
    </>
  );
}

/** `{root}/secrets/actions[/new|/NAME]` and `{root}/variables/actions[/new|/NAME]`. */
function SecretsSettings({ scope, root, kind, extra, orgRepos }: { scope: Scope; root: string; kind: "secrets" | "variables"; extra?: React.ReactNode; orgRepos?: boolean }) {
  const back = `${root}/${kind}/actions`;
  const sub = useSubRoute(root, [`/${kind}/actions`, `/${kind}/actions/new`, `/${kind}/actions/:item`]);
  if (!sub) return <NotFoundView />;
  if (sub.pattern.endsWith("/new")) return kind === "secrets" ? <SecretForm scope={scope} back={back} orgRepos={orgRepos} /> : <VariableForm scope={scope} back={back} orgRepos={orgRepos} />;
  const item = sub.params.item;
  if (item) {
    return kind === "secrets"
      ? <SecretForm key={item} scope={scope} existing={item} back={back} orgRepos={orgRepos} />
      : <VariableForm key={item} scope={scope} existing={item} back={back} orgRepos={orgRepos} />;
  }
  return (
        <>
          <h2 className="h3">Actions secrets and variables</h2>
          <p className="muted">
            {kind === "secrets"
              ? "Secrets are encrypted and are used for sensitive data. Anyone with collaborator access to this repository can use these secrets for Actions; they are not passed to workflows triggered by a pull request from a fork."
              : "Variables are shown as plain text and are used for non-sensitive data. Anyone with collaborator access to this repository can use these variables for Actions."}
          </p>
          <SecretsTabs root={root} />
          {kind === "secrets"
            ? <SecretsList scope={scope} newPath={`${back}/new`} editPath={(n) => `${back}/${encodeURIComponent(n)}`} />
            : <VariablesList scope={scope} newPath={`${back}/new`} editPath={(n) => `${back}/${encodeURIComponent(n)}`} />}
          {extra}
        </>
  );
}

// ---------- repository settings ----------

const repoScope = (ctx: RepoCtx): Scope => ({ kind: "repo", owner: ctx.owner, repo: ctx.name });

function RepoActionsSettings({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Actions settings · ${ctx.repo.full_name}`);
  return <ActionsSettings scope={repoScope(ctx)} root={`${ctx.base}/settings`} ownerName={ctx.owner} />;
}

function RepoSecrets({ ctx, kind }: { ctx: RepoCtx; kind: "secrets" | "variables" }) {
  useTitle(`Actions ${kind} · ${ctx.repo.full_name}`);
  const isOrg = ctx.repo.owner.type === "Organization";
  const extra = (
    <>
      <EnvironmentLinks ctx={ctx} />
      {isOrg && (kind === "secrets"
        ? <InheritedList title="Organization secrets" empty="There are no organization secrets available to this repository." load={() => A.repoOrgSecrets(ctx.owner, ctx.name).then((r) => r.secrets)} />
        : <InheritedList title="Organization variables" empty="There are no organization variables available to this repository." load={() => A.repoOrgVariables(ctx.owner, ctx.name).then((r) => r.variables)} />)}
    </>
  );
  return <SecretsSettings scope={repoScope(ctx)} root={`${ctx.base}/settings`} kind={kind} extra={extra} />;
}

function EnvironmentLinks({ ctx }: { ctx: RepoCtx }) {
  return (
    <div className="box">
      <div className="box-header act-box-head">
        <strong>Environment secrets and variables</strong>
        <NavLink className="btn btn-sm" to={`${ctx.base}/settings/environments`}>Manage environments</NavLink>
      </div>
      <div className="box-body muted small">Environment secrets and variables are managed on each environment's settings page.</div>
    </div>
  );
}

// ---------- organization settings ----------

const orgScope = (ctx: OrgCtx): Scope => ({ kind: "org", org: ctx.login });
const orgRoot = (ctx: OrgCtx) => `/organizations/${ctx.login}/settings`;

function OrgActionsSettings({ ctx }: { ctx: OrgCtx }) {
  useTitle(`Actions settings · ${ctx.login}`);
  return <ActionsSettings scope={orgScope(ctx)} root={orgRoot(ctx)} ownerName={ctx.login} groups />;
}

function OrgSecrets({ ctx, kind }: { ctx: OrgCtx; kind: "secrets" | "variables" }) {
  useTitle(`Actions ${kind} · ${ctx.login}`);
  return <SecretsSettings scope={orgScope(ctx)} root={orgRoot(ctx)} kind={kind} orgRepos />;
}

// ---------- site admin (enterprise) ----------

function AdminActionsSettings({ ctx }: { ctx: AdminCtx }) {
  useTitle("Actions · Site admin");
  // The admin area's AdminCtx carries the enterprise slug (`enterprise`); fall back to the default.
  const slug = (ctx as AdminCtx & { enterprise?: string }).enterprise || ENTERPRISE;
  return <ActionsSettings scope={{ kind: "enterprise", slug }} root="/site-admin" ownerName={slug} groups />;
}

export const area: Area = {
  name: "actions",
  repoTabs: [{ key: "actions", label: "Actions", icon: PlayIcon, path: "actions", order: 40 }],
  repoRoutes: (
    <>
      <Route path="actions/*" element={<ActionsRoot />} />
      <Route path="pull/:number/checks" element={<PullChecksPage />} />
      <Route path="deployments" element={<DeploymentsPage />} />
      <Route path="deployments/*" element={<DeploymentsPage />} />
    </>
  ),
  repoSettings: [
    { key: "actions", path: "actions", label: "Actions", group: "Code and automation", order: 300, element: RepoActionsSettings },
    { key: "environments", path: "environments", label: "Environments", group: "Code and automation", order: 330, element: EnvironmentsSection },
    { key: "secrets-actions", path: "secrets/actions", label: "Actions secrets", group: "Security", order: 720, element: ({ ctx }) => <RepoSecrets ctx={ctx} kind="secrets" /> },
    { key: "variables-actions", path: "variables/actions", label: "Actions variables", group: "Security", order: 721, element: ({ ctx }) => <RepoSecrets ctx={ctx} kind="variables" /> },
  ],
  orgSettings: [
    { key: "actions", path: "actions", label: "Actions", group: "Code, planning, and automation", order: 300, element: OrgActionsSettings, show: (c) => c.isOwner },
    { key: "secrets-actions", path: "secrets/actions", label: "Actions secrets", group: "Security", order: 720, element: ({ ctx }) => <OrgSecrets ctx={ctx} kind="secrets" />, show: (c) => c.isOwner },
    { key: "variables-actions", path: "variables/actions", label: "Actions variables", group: "Security", order: 721, element: ({ ctx }) => <OrgSecrets ctx={ctx} kind="variables" />, show: (c) => c.isOwner },
  ],
  adminSections: [
    { key: "actions", path: "actions", label: "Actions", group: "Policies", order: 400, element: AdminActionsSettings, show: (c) => !!c.user.site_admin },
  ],
  slots: {
    "pull.merge.top": [PullChecksBox],
    "commit.header": [CommitChecks],
    "repo.code.sidebar": [DeploymentsSidebar],
  },
};

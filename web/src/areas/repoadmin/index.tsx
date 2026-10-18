// The repoadmin area: repository settings sections, rulesets (repo / org / enterprise), webhooks,
// invitations, custom properties, Pages, and the Insights tab.
import { useEffect, useRef } from "react";
import { Link, Navigate, Route, useLocation } from "react-router-dom";
import { request } from "../../api";
import { Async, NotFoundView } from "../../components/common";
import { useRepo, type RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import type { AdminCtx, Area, OrgCtx, SettingsSection } from "../types";
import { AccessSettings, InvitationsPage, PendingInvitationsBox, RepoInvitationPage } from "./access";
import { type RepoFull } from "./api";
import { BranchesSettings } from "./branches";
import { DangerExtras, GeneralExtras } from "./general";
import {
  CodeFrequencyPage, CommitActivityPage, CommunityPage, ContributorsPage, ForksPage, PulsePage, PunchCardPage, TrafficPage,
} from "./insights";
import { Autolinks, DeployKeys, GeneratePage, OrgRepoPropertyValues, PagesSettings, PropertyDefinitions, RepoProperties } from "./misc";
import { BypassList, BypassRequestPage, RulesSection, TagRulesets, type RulesLevel } from "./rulesets";
import { HooksSection } from "./webhooks";
import "./styles.css";

const enc = encodeURIComponent;

const repoLevel = (ctx: RepoCtx): RulesLevel => ({
  kind: "repo",
  api: `/repos/${enc(ctx.owner)}/${enc(ctx.name)}`,
  root: `${ctx.base}/settings/rules`,
  org: ctx.repo.owner.type === "Organization" ? ctx.owner : undefined,
  owner: ctx.owner,
  repo: ctx.name,
  repoId: ctx.repo.id,
});
const orgLevel = (ctx: OrgCtx): RulesLevel => ({ kind: "org", api: `/orgs/${enc(ctx.login)}`, root: `/organizations/${ctx.login}/settings/rules`, org: ctx.login });

// ---------- enterprise slug ----------

let slugCache: Promise<string> | null = null;
export function enterpriseSlug(): Promise<string> {
  slugCache ??= request<{ slug: string }>("/api/forge/enterprise").then((r) => r.slug, () => "forge");
  return slugCache;
}
/** The enterprise slug: from the admin shell's context when it provides one, else /api/forge/enterprise. */
function useEnterprise(ctx?: AdminCtx) {
  const given = (ctx as (AdminCtx & { enterprise?: string }) | undefined)?.enterprise;
  return useAsync(() => (given ? Promise.resolve(given) : enterpriseSlug()), [given]);
}

// ---------- repo settings sections ----------

const repoSettings: SettingsSection<RepoCtx>[] = [
  { key: "access", path: "access", label: "Collaborators and teams", group: "Access", order: 100, element: AccessSettings },
  { key: "branches", path: "branches", label: "Branches", group: "Code and automation", order: 200, element: BranchesSettings },
  { key: "tags", path: "tags", label: "Tags", group: "Code and automation", order: 210, element: ({ ctx }) => <TagRulesets level={{ ...repoLevel(ctx), root: `${ctx.base}/settings/rules` }} /> },
  { key: "rules", path: "rules", label: "Rules", group: "Code and automation", order: 220, element: ({ ctx }) => <RulesSection level={repoLevel(ctx)} /> },
  { key: "hooks", path: "hooks", label: "Webhooks", group: "Code and automation", order: 250,
    element: ({ ctx }) => <HooksSection level={{ kind: "repo", api: `/repos/${enc(ctx.owner)}/${enc(ctx.name)}`, root: `${ctx.base}/settings/hooks` }} /> },
  { key: "pages", path: "pages", label: "Pages", group: "Code and automation", order: 270, element: PagesSettings },
  { key: "custom-properties", path: "custom-properties", label: "Custom properties", group: "Code and automation", order: 280, element: RepoProperties,
    show: (ctx) => ctx.repo.owner.type === "Organization" },
  { key: "keys", path: "keys", label: "Deploy keys", group: "Security", order: 320, element: DeployKeys },
  { key: "key_links", path: "key_links", label: "Autolink references", group: "Integrations", order: 400, element: Autolinks },
];

// ---------- org settings sections (rendered by the orgs area's shell) ----------

function OrgOnly({ ctx, children }: { ctx: OrgCtx; children: React.ReactNode }) {
  if (!ctx.isOwner) return <NotFoundView />;
  return <>{children}</>;
}

const orgSettings: SettingsSection<OrgCtx>[] = [
  { key: "repoadmin-rules", path: "rules", label: "Rulesets", group: "Code, planning, and automation", order: 300,
    element: ({ ctx }) => <OrgOnly ctx={ctx}><RulesSection level={orgLevel(ctx)} /></OrgOnly>, show: (c) => c.isOwner },
  { key: "repoadmin-hooks", path: "hooks", label: "Webhooks", group: "Code, planning, and automation", order: 320,
    element: ({ ctx }) => <OrgOnly ctx={ctx}><HooksSection level={{ kind: "org", api: `/orgs/${enc(ctx.login)}`, root: `/organizations/${ctx.login}/settings/hooks` }} /></OrgOnly>,
    show: (c) => c.isOwner },
  { key: "repoadmin-properties", path: "custom-properties", label: "Custom properties", group: "Repository", order: 340,
    element: ({ ctx }) => (
      <OrgOnly ctx={ctx}>
        <PropertyDefinitions api={`/orgs/${enc(ctx.login)}`} />
        <OrgRepoPropertyValues org={ctx.login} />
      </OrgOnly>
    ), show: (c) => c.isOwner },
];

// ---------- site admin sections (rendered by the admin area's shell) ----------

function EnterpriseRules({ ctx }: { ctx: AdminCtx }) {
  const e = useEnterprise(ctx);
  return <Async state={e}>{(slug) => <RulesSection section="rulesets" level={{ kind: "enterprise", api: `/enterprises/${enc(slug)}`, root: "/site-admin/rulesets" }} />}</Async>;
}
function EnterpriseProperties({ ctx }: { ctx: AdminCtx }) {
  const e = useEnterprise(ctx);
  return <Async state={e}>{(slug) => <PropertyDefinitions api={`/enterprises/${enc(slug)}`} title="Enterprise custom properties" />}</Async>;
}
function EnterpriseBypass({ ctx }: { ctx: AdminCtx }) {
  const e = useEnterprise(ctx);
  return <Async state={e}>{(slug) => <div className="settings"><BypassList level={{ kind: "enterprise", api: `/enterprises/${enc(slug)}`, root: "/site-admin/bypass-requests" }} /></div>}</Async>;
}

const adminSections: SettingsSection<AdminCtx>[] = [
  { key: "repoadmin-rulesets", path: "rulesets", label: "Rulesets", group: "Policies", order: 600, element: EnterpriseRules },
  { key: "repoadmin-properties", path: "custom-properties", label: "Custom properties", group: "Policies", order: 610, element: EnterpriseProperties },
  { key: "repoadmin-bypass", path: "bypass-requests", label: "Push bypass requests", group: "Policies", order: 620, element: EnterpriseBypass },
];

// ---------- repo routes ----------

function ExemptionsPage() {
  const ctx = useRepo();
  useTitle(`Bypass requests · ${ctx.repo.full_name}`);
  if (!ctx.canPush) return <NotFoundView />;
  return <div className="settings ra-exemptions"><BypassList level={{ ...repoLevel(ctx), root: `${ctx.base}/exemptions` }} /></div>;
}
function ExemptionPage() {
  const ctx = useRepo();
  if (!ctx.canPush) return <NotFoundView />;
  return <BypassRequestPage owner={ctx.owner} repo={ctx.name} canReview={ctx.canPush} />;
}

// ---------- slots ----------

/** Repository header: "Public archive" badge, "Use this template" and a traffic beacon. */
function RepoHeaderExtras({ ctx }: { ctx: RepoCtx }) {
  const repo = ctx.repo as RepoFull;
  const loc = useLocation();
  const last = useRef("");
  useEffect(() => {
    // Page view for repository traffic (views, popular content, referrers).
    if (last.current === loc.pathname) return;
    const first = last.current === "";
    last.current = loc.pathname;
    const ref = first && document.referrer && !document.referrer.startsWith(location.origin) ? document.referrer : "";
    fetch(`/api/forge/repos/${enc(ctx.owner)}/${enc(ctx.name)}/traffic/view`, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", "X-Forge-CSRF": "1" },
      body: JSON.stringify({ path: loc.pathname, title: document.title, referrer: ref }),
      keepalive: true,
    }).catch(() => {});
  }, [loc.pathname, ctx.owner, ctx.name]);
  return (
    <>
      {repo.archived && <span className="badge badge-warn" data-testid="archived-badge">Public archive</span>}
      {repo.is_template && (
        <Link className="btn btn-sm btn-primary" to={`${ctx.base}/generate`} data-testid="use-template">Use this template</Link>
      )}
    </>
  );
}

function InsightsIndex() {
  const ctx = useRepo();
  return <Navigate to={`${ctx.base}/pulse`} replace />;
}

const GraphIcon = ({ size = 16, className = "icon" }: { size?: number; className?: string }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round" aria-hidden="true" className={className}>
    <path d="M2 14h12M4 11V7M8 11V3M12 11V6" />
  </svg>
);

export const area: Area = {
  name: "repoadmin",
  topRoutes: (
    <>
      <Route path="invitations" element={<InvitationsPage />} />
      <Route path=":owner/:repo/invitations" element={<RepoInvitationPage />} />
    </>
  ),
  repoRoutes: (
    <>
      <Route path="pulse" element={<PulsePage />} />
      <Route path="graphs" element={<InsightsIndex />} />
      <Route path="graphs/contributors" element={<ContributorsPage />} />
      <Route path="graphs/commit-activity" element={<CommitActivityPage />} />
      <Route path="graphs/code-frequency" element={<CodeFrequencyPage />} />
      <Route path="graphs/punch-card" element={<PunchCardPage />} />
      <Route path="graphs/traffic" element={<TrafficPage />} />
      <Route path="forks" element={<ForksPage />} />
      <Route path="network/members" element={<ForksPage />} />
      <Route path="community" element={<CommunityPage />} />
      <Route path="exemptions" element={<ExemptionsPage />} />
      <Route path="exemptions/:number" element={<ExemptionPage />} />
      <Route path="generate" element={<GeneratePage />} />
    </>
  ),
  repoTabs: [{ key: "insights", label: "Insights", icon: GraphIcon, path: "pulse", order: 70, alsoActive: ["/graphs", "/forks", "/community", "/network"] }],
  repoSettings,
  orgSettings,
  adminSections,
  userMenu: [{ key: "repoadmin-invitations", label: "Repository invitations", to: "/invitations", order: 300 }],
  slots: {
    "repo.settings.general": [GeneralExtras],
    "repo.settings.danger": [DangerExtras],
    "repo.header.actions": [RepoHeaderExtras],
    "home.sidebar": [PendingInvitationsBox],
  },
};


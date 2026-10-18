// The admin area: site administration at /site-admin/* (a shell other areas add sections to via
// `adminSections`), the "Site admin" user-menu entry, organization and repository "Hooks"
// settings sections (pre-receive hook enforcement) and the maintenance-mode experience.
import { Route } from "react-router-dom";
import type { AdminCtx, Area, SettingsSection } from "../types";
import { KeysSection, OrgsSection, ReposSection, TokensSection } from "./Accounts";
import { AuditLogSection, MigrationsSection } from "./Compliance";
import { GlobalHooksSection } from "./Hooks";
import { LdapSection, ScimSection } from "./Identity";
import { MaintenanceWatch } from "./MaintenanceWatch";
import { LicenseSection, NodesSection, SettingsSection as ManageSettingsSection, SshAccessSection } from "./Manage";
import { AnnouncementSection, MaintenanceSection } from "./Messages";
import { OverviewSection } from "./Overview";
import { OrgHooksSection, PreReceiveEnvsSection, PreReceiveHooksSection, RepoHooksSection } from "./PreReceive";
import { SiteAdminPage } from "./Shell";
import { EnterpriseTeamsSection } from "./Teams";
import { UsersSection } from "./Users";
import "./styles.css";

/**
 * Site admin sidebar groups and orders used by this area (other areas: pick a group below and
 * an order in its range, e.g. actions → "Code and automation" 430–450, security → "Security" 700+).
 */
const sections: SettingsSection<AdminCtx>[] = [
  { key: "overview", path: "overview", label: "Overview", order: 0, element: OverviewSection },
  { key: "users", path: "users", label: "Users", group: "Accounts", order: 100, element: UsersSection },
  { key: "organizations", path: "organizations", label: "Organizations", group: "Accounts", order: 110, element: OrgsSection },
  { key: "repositories", path: "repositories", label: "Repositories", group: "Accounts", order: 120, element: ReposSection },
  { key: "enterprise-teams", path: "enterprise-teams", label: "Enterprise teams", group: "Accounts", order: 130, element: EnterpriseTeamsSection },
  { key: "keys", path: "keys", label: "SSH keys", group: "Accounts", order: 140, element: KeysSection },
  { key: "tokens", path: "tokens", label: "Access tokens", group: "Accounts", order: 150, element: TokensSection },
  { key: "announcement", path: "announcement", label: "Announcement", group: "Messages", order: 200, element: AnnouncementSection },
  { key: "maintenance", path: "maintenance", label: "Maintenance mode", group: "Management console", order: 300, element: MaintenanceSection },
  { key: "settings", path: "settings", label: "Settings", group: "Management console", order: 310, element: ManageSettingsSection },
  { key: "nodes", path: "nodes", label: "Nodes and health", group: "Management console", order: 320, element: NodesSection },
  { key: "license", path: "license", label: "License", group: "Management console", order: 330, element: LicenseSection },
  { key: "ssh-access", path: "ssh-access", label: "Administrative SSH", group: "Management console", order: 340, element: SshAccessSection },
  { key: "hooks", path: "hooks", label: "Global webhooks", group: "Code and automation", order: 400, element: GlobalHooksSection },
  { key: "pre-receive-hooks", path: "pre-receive-hooks", label: "Pre-receive hooks", group: "Code and automation", order: 410, element: PreReceiveHooksSection },
  { key: "pre-receive-environments", path: "pre-receive-environments", label: "Pre-receive environments", group: "Code and automation", order: 420, element: PreReceiveEnvsSection },
  { key: "ldap", path: "ldap", label: "LDAP", group: "Authentication", order: 500, element: LdapSection },
  { key: "scim", path: "scim", label: "SCIM", group: "Authentication", order: 510, element: ScimSection },
  { key: "audit-log", path: "audit-log", label: "Audit log", group: "Compliance", order: 600, element: AuditLogSection },
  { key: "live-migrations", path: "live-migrations", label: "Live migrations", group: "Compliance", order: 610, element: MigrationsSection },
];

export const area: Area = {
  name: "admin",
  topRoutes: (
    <>
      <Route path="site-admin" element={<SiteAdminPage />} />
      {/* Two-segment URLs need their own route: "site-admin/*" ranks below "/:owner/:repo" (index). */}
      <Route path="site-admin/:section" element={<SiteAdminPage />} />
      <Route path="site-admin/*" element={<SiteAdminPage />} />
    </>
  ),
  adminSections: sections,
  userMenu: [{ key: "site-admin", label: "Site admin", to: "/site-admin", order: 900, show: (me) => !!me.site_admin }],
  // Path "pre-receive-hooks" (not "hooks", which is the webhooks page owned by repoadmin).
  orgSettings: [{ key: "pre-receive-hooks", path: "pre-receive-hooks", label: "Pre-receive hooks", group: "Code and automation", order: 265, element: OrgHooksSection, show: (c) => c.isOwner }],
  repoSettings: [{ key: "pre-receive-hooks", path: "pre-receive-hooks", label: "Pre-receive hooks", group: "Code and automation", order: 265, element: RepoHooksSection, show: (c) => c.isAdmin }],
  slots: { "header.right": [MaintenanceWatch] },
};

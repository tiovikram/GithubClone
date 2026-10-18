import { lazy, Suspense } from "react";
import { Route } from "react-router-dom";
import { Spinner } from "../../components/common";
import type { Area } from "../types";
import { AppInstallPage, AppPublicPage, AppsSection } from "./apps";
import { PackageIcon } from "./icons";
import { ApplicationsSection, DevicePage, OAuthAppsSection, OrgInstallationsSection } from "./oauth";
import {
  DeletedPackagesSection, OrgPackagesPage, PackageRoute, ProfilePackagesTab, RepoPackagesPage, RepoPackagesSidebar,
  UserPackagesRedirect, UserPackagesSettings,
} from "./packages";
import { PrivateRegistriesSection } from "./registries";
import "./styles.css";

// `/settings/apps` etc. would otherwise match `/:owner/:repo` (it outranks `settings/*`).
// Loaded lazily: the core settings page imports the area registry, which imports this file.
const LazySettings = lazy(() => import("../../pages/UserSettings").then((m) => ({ default: m.UserSettingsPage })));
const Settings = () => <Suspense fallback={<Spinner />}><LazySettings /></Suspense>;

const userRoot = { root: "/settings", isOrg: false };
const orgOwner = (login: string) => ({ login, isOrg: true, root: `/organizations/${login}/settings` });

// The packages area: packages (user / org / repo), GitHub Apps, OAuth apps and their
// authorizations, the device-flow page and organization private registries.
export const area: Area = {
  name: "packages",
  topRoutes: (
    <>
      <Route path="users/:owner/packages" element={<UserPackagesRedirect />} />
      <Route path="users/:owner/packages/:ptype/*" element={<PackageRoute />} />
      <Route path="orgs/:org/packages" element={<OrgPackagesPage />} />
      <Route path="orgs/:owner/packages/:ptype/*" element={<PackageRoute />} />
      <Route path="apps/:slug" element={<AppPublicPage />} />
      <Route path="apps/:slug/installations/new" element={<AppInstallPage step="account" />} />
      <Route path="apps/:slug/installations/new/permissions" element={<AppInstallPage step="permissions" />} />
      <Route path="login/device" element={<DevicePage />} />
      {["packages", "installations", "apps", "applications"].map((p) => <Route key={p} path={`settings/${p}`} element={<Settings />} />)}
    </>
  ),
  repoRoutes: <Route path="packages" element={<RepoPackagesPage />} />,
  profileTabs: [{ key: "packages", label: "Packages", order: 30, icon: PackageIcon, element: ProfilePackagesTab }],
  slots: { "repo.code.sidebar": [RepoPackagesSidebar] },
  userSettings: [
    { key: "packages", path: "packages", label: "Packages", group: "Code, planning, and automation", order: 650, element: UserPackagesSettings },
    { key: "installations", path: "installations", label: "Applications", group: "Integrations", order: 700, element: ApplicationsSection },
    { key: "apps", path: "apps", label: "GitHub Apps", group: "Developer settings", order: 2000, element: ({ ctx }) => <AppsSection owner={{ ...userRoot, login: ctx.user.login }} /> },
    { key: "oauth-apps", path: "applications", label: "OAuth Apps", group: "Developer settings", order: 2010, element: ({ ctx }) => <OAuthAppsSection owner={{ ...userRoot, login: ctx.user.login }} /> },
  ],
  orgSettings: [
    { key: "packages", path: "packages", label: "Packages", group: "Code, planning, and automation", order: 650, show: (c) => c.isOwner, element: ({ ctx }) => <DeletedPackagesSection owner={ctx.org} /> },
    { key: "private-registries", path: "private-registries", label: "Private registries", group: "Code, planning, and automation", order: 660, show: (c) => c.isOwner,
      element: ({ ctx }) => <PrivateRegistriesSection org={ctx.login} isOwner={ctx.isOwner} /> },
    { key: "installations", path: "installations", label: "GitHub Apps", group: "Third-party Access", order: 700, show: (c) => c.isOwner, element: ({ ctx }) => <OrgInstallationsSection org={ctx.login} /> },
    { key: "apps", path: "apps", label: "GitHub Apps", group: "Developer settings", order: 2000, show: (c) => c.isOwner, element: ({ ctx }) => <AppsSection owner={orgOwner(ctx.login)} /> },
    { key: "oauth-apps", path: "applications", label: "OAuth Apps", group: "Developer settings", order: 2010, show: (c) => c.isOwner, element: ({ ctx }) => <OAuthAppsSection owner={orgOwner(ctx.login)} /> },
  ],
};

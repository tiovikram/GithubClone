// Extension points for feature areas. Each area (src/areas/<name>/index.tsx) exports one `Area`;
// the core shell (App, Layout, RepoLayout, settings pages, PR/issue/commit pages) renders
// whatever the areas register, so areas never edit each other's files or the core pages.
import type { ComponentType, ReactNode } from "react";
import type { Issue, Pull, Repo, User } from "../api";
import type { RepoCtx } from "../components/RepoLayout";

type Icon = ComponentType<{ size?: number; className?: string }>;

/** A tab in the repository header (after Code / Issues / Pull requests). */
export interface RepoTab {
  key: string;
  label: string;
  icon?: Icon;
  /** Path under the repo base, e.g. "actions" → /{o}/{r}/actions. */
  path: string;
  /** Sort key; core tabs: Code 0, Issues 10, Pull requests 20, Releases 80, Settings 1000. */
  order: number;
  /** Other path prefixes (under the repo base) that keep the tab selected, e.g. ["/runs/"]. */
  alsoActive?: string[];
  show?: (ctx: RepoCtx) => boolean;
}

/** A page in a settings sidebar (user, repository, organization or site admin settings). */
export interface SettingsSection<C> {
  key: string;
  /** Path segment(s) under the settings root, e.g. "secrets/actions". */
  path: string;
  label: string;
  /** Sidebar group heading, e.g. "Security", "Code and automation". Ungrouped sections come first. */
  group?: string;
  order: number;
  element: ComponentType<{ ctx: C }>;
  show?: (ctx: C) => boolean;
}

export interface UserSettingsCtx {
  user: User;
}

/** Context passed to organization settings sections (the orgs area renders the shell). */
export interface OrgCtx {
  org: User;
  login: string;
  /** The viewer is an owner (admin) of the organization. */
  isOwner: boolean;
  reload: () => void;
}

/** Context passed to site admin sections (the admin area renders the shell). */
export interface AdminCtx {
  user: User;
}

export interface MenuLink {
  key: string;
  label: string;
  to: string | ((me: User) => string);
  order: number;
  show?: (me: User) => boolean;
}

export interface ProfileTab {
  key: string;
  label: string;
  order: number;
  icon?: Icon;
  element: ComponentType<{ user: User }>;
  show?: (user: User, me: User | null) => boolean;
}

/** Props each slot passes to its components. */
export interface SlotProps {
  /** Global header, left of the theme toggle (e.g. notifications bell). */
  "header.right": { user: User | null };
  /** Right side of the repository title row (next to Fork / Star). */
  "repo.header.actions": { ctx: RepoCtx };
  /** Below the About box on the repository Code page. */
  "repo.code.sidebar": { ctx: RepoCtx; refName: string };
  /** Above the merge box on the PR conversation tab (e.g. checks summary). */
  "pull.merge.top": { ctx: RepoCtx; pull: Pull };
  /** PR conversation sidebar, below labels. */
  "pull.sidebar": { ctx: RepoCtx; pull: Pull; setPull: (p: Pull) => void };
  /** Issue sidebar, below assignees. */
  "issue.sidebar": { ctx: RepoCtx; issue: Issue; setIssue: (i: Issue) => void };
  /** Commit page, below the commit header (e.g. checks / statuses). */
  "commit.header": { ctx: RepoCtx; sha: string };
  /** Dashboard left column, below "Top repositories". */
  "home.sidebar": { user: User };
  /** Profile sidebar, below the user details. */
  "profile.sidebar": { user: User; me: User | null };
  /** Repository row extras on the repository list (rare). */
  "repo.list.meta": { repo: Repo };
}
export type SlotName = keyof SlotProps;

export interface Area {
  name: string;
  /** <Route> elements under the global layout, e.g. <Route path="orgs/:org" …/>. */
  topRoutes?: ReactNode;
  /** <Route> elements under /:owner/:repo, e.g. <Route path="actions/*" …/>. */
  repoRoutes?: ReactNode;
  repoTabs?: RepoTab[];
  repoSettings?: SettingsSection<RepoCtx>[];
  userSettings?: SettingsSection<UserSettingsCtx>[];
  orgSettings?: SettingsSection<OrgCtx>[];
  adminSections?: SettingsSection<AdminCtx>[];
  /** Links in the global header next to "Dashboard". */
  headerNav?: MenuLink[];
  /** Links in the user (avatar) menu. */
  userMenu?: MenuLink[];
  profileTabs?: ProfileTab[];
  /** Replaces the profile page for organization accounts (provided by the orgs area). */
  orgProfile?: ComponentType<{ org: User }>;
  slots?: { [K in SlotName]?: ComponentType<SlotProps[K]>[] };
}

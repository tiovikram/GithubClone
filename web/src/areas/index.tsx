// The registry of feature areas. One import per area; each area only edits its own directory.
import { Fragment, type ReactNode } from "react";
import type { Area, SettingsSection, SlotName, SlotProps } from "./types";
import { area as actions } from "./actions";
import { area as admin } from "./admin";
import { area as orgs } from "./orgs";
import { area as packages } from "./packages";
import { area as planning } from "./planning";
import { area as repoadmin } from "./repoadmin";
import { area as security } from "./security";
import { area as social } from "./social";

export const areas: Area[] = [planning, repoadmin, actions, security, orgs, packages, social, admin];

const byOrder = <T extends { order: number }>(a: T, b: T) => a.order - b.order;

export const repoTabs = areas.flatMap((a) => a.repoTabs || []).sort(byOrder);
export const repoSettings = areas.flatMap((a) => a.repoSettings || []).sort(byOrder);
export const userSettings = areas.flatMap((a) => a.userSettings || []).sort(byOrder);
export const orgSettings = areas.flatMap((a) => a.orgSettings || []).sort(byOrder);
export const adminSections = areas.flatMap((a) => a.adminSections || []).sort(byOrder);
export const headerNav = areas.flatMap((a) => a.headerNav || []).sort(byOrder);
export const userMenu = areas.flatMap((a) => a.userMenu || []).sort(byOrder);
export const profileTabs = areas.flatMap((a) => a.profileTabs || []).sort(byOrder);
export const orgProfile = areas.find((a) => a.orgProfile)?.orgProfile;

export function topRoutes(): ReactNode {
  return areas.map((a) => <Fragment key={a.name}>{a.topRoutes}</Fragment>);
}

export function repoRoutes(): ReactNode {
  return areas.map((a) => <Fragment key={a.name}>{a.repoRoutes}</Fragment>);
}

/** Renders every component registered for a slot, in area order. */
export function Slot<K extends SlotName>({ name, ...props }: { name: K } & SlotProps[K]) {
  const comps = areas.flatMap((a) => (a.slots?.[name] || []) as React.ComponentType<SlotProps[K]>[]);
  if (!comps.length) return null;
  return <>{comps.map((C, i) => <C key={i} {...(props as unknown as SlotProps[K])} />)}</>;
}

/** Sections visible for `ctx`, grouped in first-seen group order (ungrouped first). */
export function groupSections<C>(sections: SettingsSection<C>[], ctx: C): [string, SettingsSection<C>[]][] {
  const visible = sections.filter((s) => !s.show || s.show(ctx));
  const groups = new Map<string, SettingsSection<C>[]>();
  for (const s of visible.filter((s) => !s.group)) groups.set("", [...(groups.get("") || []), s]);
  for (const s of visible.filter((s) => s.group)) groups.set(s.group!, [...(groups.get(s.group!) || []), s]);
  return [...groups.entries()];
}

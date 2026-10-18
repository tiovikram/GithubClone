import { NavLink, useParams } from "react-router-dom";
import { groupSections } from "../areas";
import type { SettingsSection } from "../areas/types";
import { NotFoundView } from "./common";

/** Pick the section whose path is the longest prefix of `splat` ("" = the first section). */
export function pickSection<C>(sections: SettingsSection<C>[], ctx: C, splat: string): SettingsSection<C> | undefined {
  const visible = sections.filter((s) => !s.show || s.show(ctx));
  const p = splat.replace(/^\/+|\/+$/g, "");
  if (!p) return visible[0];
  return visible
    .filter((s) => p === s.path || p.startsWith(`${s.path}/`))
    .sort((a, b) => b.path.length - a.path.length)[0];
}

/** Sidebar for a settings area: links are `${root}/${section.path}`. */
export function SettingsSidebar<C>({ root, sections, ctx, label = "Settings", active }: {
  root: string;
  sections: SettingsSection<C>[];
  ctx: C;
  label?: string;
  active?: SettingsSection<C>;
}) {
  const cls = (s: SettingsSection<C>) => ({ isActive }: { isActive: boolean }) =>
    "menu-item" + (isActive || active?.key === s.key ? " selected" : "");
  return (
    <nav className="settings-nav" aria-label={label}>
      {groupSections(sections, ctx).map(([group, list]) => (
        <div key={group || "_"} className="settings-nav-group">
          {group && <h2 className="settings-nav-heading">{group}</h2>}
          {list.map((s) => (
            <NavLink key={s.key} to={`${root}/${s.path}`} end={false} className={cls(s)} data-testid={`settings-nav-${s.key}`}>
              {s.label}
            </NavLink>
          ))}
        </div>
      ))}
    </nav>
  );
}

/** A complete settings page: sidebar + the section matched by the route splat. */
export function SettingsShell<C>({ root, sections, ctx, label, container = true }: {
  root: string;
  sections: SettingsSection<C>[];
  ctx: C;
  label?: string;
  /** Wrap in `.container` (top-level pages) or not (inside the repository layout). */
  container?: boolean;
}) {
  const { "*": splat = "" } = useParams();
  const section = pickSection(sections, ctx, splat);
  const El = section?.element;
  return (
    <div className={(container ? "container " : "") + "settings-layout"}>
      <SettingsSidebar root={root} sections={sections} ctx={ctx} label={label} active={section} />
      <div className="settings-main">{El ? <El ctx={ctx} /> : <NotFoundView />}</div>
    </div>
  );
}

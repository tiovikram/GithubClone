import { useLocation, useParams } from "react-router-dom";

/** The path under the settings root (`/settings/` or `/organizations/{org}/settings/`), from the
 * pathname: settings pages are reached through `settings/:section`, `settings/:section/*` and
 * `.../settings/*` routes, whose params differ. */
export function useSettingsSplat(): string {
  const { "*": star } = useParams();
  const { pathname } = useLocation();
  const m = pathname.match(/^\/(?:settings|organizations\/[^/]+\/settings)\/(.*)$/);
  return m ? m[1] : star ?? "";
}

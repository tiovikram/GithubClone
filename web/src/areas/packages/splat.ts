import { useLocation, useParams } from "react-router-dom";

/** The path under the settings root (`/settings/` or `/organizations/{org}/settings/`). Two-segment
 * URLs such as `/settings/apps` are matched by explicit routes without a splat, so fall back to
 * the pathname. */
export function useSettingsSplat(): string {
  const { "*": star } = useParams();
  const { pathname } = useLocation();
  if (star !== undefined) return star;
  const m = pathname.match(/^\/(?:settings|organizations\/[^/]+\/settings)\/(.*)$/);
  return m ? m[1] : "";
}

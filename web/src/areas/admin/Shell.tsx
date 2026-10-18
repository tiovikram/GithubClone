// The site admin shell (`/site-admin/*`): a settings-style sidebar of every `adminSections`
// entry registered by any area, rendered only for site administrators (404 otherwise).
import { Link, useLocation } from "react-router-dom";
import { adminSections } from "..";
import type { AdminCtx } from "../types";
import { useAuth } from "../../auth";
import { ErrorBox, NotFoundView, Spinner } from "../../components/common";
import { pickSection, SettingsSidebar } from "../../components/SettingsShell";
import { useAsync, useTitle } from "../../hooks";
import { adminApi } from "./api";

export function SiteAdminPage() {
  const { user, loading } = useAuth();
  const { pathname } = useLocation();
  const isAdmin = !!user?.site_admin;
  const info = useAsync(() => (isAdmin ? adminApi.info() : Promise.resolve(null)), [isAdmin]);
  useTitle("Site admin");
  if (loading) return <Spinner />;
  if (!user || !isAdmin) return <NotFoundView />;
  if (info.error) return <div className="container"><ErrorBox error={info.error} onRetry={info.reload} /></div>;
  if (!info.data) return <Spinner />;
  const ctx: AdminCtx = { user, enterprise: info.data.enterprise_slug };
  return (
    <div className="site-admin" data-testid="site-admin">
      <div className="sa-titlebar">
        <div className="container sa-titlebar-inner">
          <Link to="/site-admin" className="sa-titlebar-title">Site admin</Link>
          <span className="muted small">Enterprise <strong>{ctx.enterprise}</strong> · forge {info.data.forge_version} · GHES API {info.data.ghes_api_version}</span>
        </div>
      </div>
      <AdminShell ctx={ctx} splat={pathname.replace(/^\/site-admin\/?/, "")} />
    </div>
  );
}

/**
 * `SettingsShell` (container) with the splat taken from the URL: /site-admin/<section> is served
 * by the route `site-admin/:section` (a `site-admin/*` route would lose to `/:owner/:repo`'s index
 * route for two-segment URLs), where `useParams()["*"]` is undefined.
 */
function AdminShell({ ctx, splat }: { ctx: AdminCtx; splat: string }) {
  const section = pickSection(adminSections, ctx, decodeURI(splat));
  const El = section?.element;
  return (
    <div className="container settings-layout">
      <SettingsSidebar root="/site-admin" sections={adminSections} ctx={ctx} label="Site admin" active={section} />
      <div className="settings-main">{El ? <El ctx={ctx} /> : <NotFoundView />}</div>
    </div>
  );
}

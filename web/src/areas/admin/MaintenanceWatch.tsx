// Maintenance-mode experience. Rendered in the `header.right` slot on every page: it reads
// `/api/forge/maintenance` (reachable during maintenance) on load, on navigation (at most every
// 15 s) and when the tab regains focus, and
// - shows a full-page maintenance screen to users who are locked out (the API answers 503);
// - shows site admins (who keep access) a bar reminding them that maintenance mode is on;
// - shows everyone a notice while maintenance is scheduled.
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Link, useLocation } from "react-router-dom";
import type { User } from "../../api";
import { LogoIcon } from "../../components/icons";
import { fullDate } from "../../util";
import { adminApi, type MaintenanceState } from "./api";

export function MaintenanceWatch({ user }: { user: User | null }) {
  const [m, setM] = useState<MaintenanceState | null>(null);
  const loc = useLocation();
  const last = useRef(0);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const onFocus = () => { last.current = 0; setTick((t) => t + 1); };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);
  useEffect(() => {
    last.current = 0; // signing in or out changes what the viewer may see
  }, [user?.login]);
  useEffect(() => {
    // Re-check sooner while maintenance is on or scheduled (it ends or starts without a navigation).
    const every = m && m.status !== "off" ? 5000 : 15000;
    if (Date.now() - last.current < every) return;
    last.current = Date.now();
    let live = true;
    adminApi.maintenanceState().then((s) => live && setM(s)).catch(() => undefined);
    return () => { live = false; };
  }, [loc.pathname, tick, user?.login, m]);
  useEffect(() => {
    if (!m || m.status === "off") return;
    const t = setInterval(() => setTick((x) => x + 1), 10000);
    return () => clearInterval(t);
  }, [m]);
  const locked = !!m && m.active && !m.bypass;
  const wasLocked = useRef(false);
  useEffect(() => {
    // Back from maintenance: reload so every request that got 503 runs again.
    if (wasLocked.current && m && !locked) location.reload();
    wasLocked.current = locked;
  }, [locked, m]);
  if (!m || m.status === "off") return null;
  if (locked && loc.pathname !== "/login") {
    return createPortal(
      <div className="sa-maint-page" role="alert" aria-live="assertive" data-testid="maintenance-page">
        <div className="sa-maint-card">
          <LogoIcon size={48} />
          <h1>Down for maintenance</h1>
          <p className="sa-maint-msg">{m.message || "This instance is undergoing maintenance. Please try again later."}</p>
          <p className="muted small">This page refreshes automatically when the site is back.</p>
          <div className="sa-maint-actions">
            <button type="button" className="btn" onClick={() => location.reload()}>Try again</button>
            {!user && <Link className="btn btn-link" to="/login">Site administrators: sign in</Link>}
          </div>
        </div>
      </div>,
      document.body,
    );
  }
  if (m.active) {
    if (locked) return null;
    return createPortal(
      <div className="sa-maint-bar sa-maint-bar-on" role="status" data-testid="maintenance-admin-bar">
        Maintenance mode is on — users see the maintenance page.{" "}
        {user?.site_admin && <Link to="/site-admin/maintenance">Manage</Link>}
      </div>,
      document.body,
    );
  }
  return createPortal(
    <div className="sa-maint-bar" role="status" data-testid="maintenance-scheduled-bar">
      Scheduled maintenance{m.scheduled_time ? ` starts ${fullDate(m.scheduled_time)}` : ""}.{m.message ? ` ${m.message}` : ""}
    </div>,
    document.body,
  );
}

import { Navigate } from "react-router-dom";
import { userSettings } from "../areas";
import type { SettingsSection, UserSettingsCtx } from "../areas/types";
import { useAuth } from "../auth";
import { Spinner } from "../components/common";
import { SettingsShell, SettingsSidebar } from "../components/SettingsShell";
import { useTitle } from "../hooks";

// Tokens and SSH keys are core pages with their own routes (/settings/tokens, /settings/keys)
// that render the shared sidebar themselves; areas add the other sections.
const core: SettingsSection<UserSettingsCtx>[] = [
  { key: "tokens", path: "tokens", label: "Access tokens", group: "Access", order: 500, element: () => <Navigate to="/settings/tokens" replace /> },
  { key: "keys", path: "keys", label: "SSH keys", group: "Access", order: 510, element: () => <Navigate to="/settings/keys" replace /> },
];

export const allUserSettings = [...core, ...userSettings].sort((a, b) => a.order - b.order);

/** The user settings sidebar (used by the core Tokens and Keys pages too). */
export function UserSettingsNav() {
  const { user } = useAuth();
  if (!user) return null;
  return <SettingsSidebar root="/settings" sections={allUserSettings} ctx={{ user }} label="Settings" />;
}

/** `/settings/*` for sections registered by areas. */
export function UserSettingsPage() {
  const { user, loading } = useAuth();
  useTitle("Settings");
  if (loading) return <Spinner />;
  if (!user) return <Navigate to={`/login?return_to=${encodeURIComponent(location.pathname)}`} replace />;
  return <SettingsShell root="/settings" sections={allUserSettings} ctx={{ user }} label="Settings" />;
}

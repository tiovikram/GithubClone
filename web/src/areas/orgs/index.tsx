// The orgs area: organization profile, people, teams, organization settings shell and sections,
// create organization, the user's Organizations settings and org invitations.
import { Navigate, Route, useParams } from "react-router-dom";
import type { Area } from "../types";
import { NewOrgPage } from "./NewOrg";
import { NewOrgRepoPage, OrgProfile, OrgRepositoriesPage } from "./OrgProfile";
import { PeoplePage } from "./People";
import { OrgSettingsPage, ownOrgSettings } from "./Settings";
import { NewTeamPage, TeamPage, TeamsPage } from "./Teams";
import { InvitationPage, OrganizationsSettings, PendingOrgInvites } from "./UserOrgs";
import "./styles.css";

/** github.com redirects /orgs/:org to the profile at /:org. */
function OrgRedirect() {
  const { org = "" } = useParams();
  return <Navigate to={`/${org}`} replace />;
}

export const area: Area = {
  name: "orgs",
  topRoutes: (
    <>
      <Route path="organizations/new" element={<NewOrgPage />} />
      <Route path="organizations/:org/repositories/new" element={<NewOrgRepoPage />} />
      <Route path="organizations/:org/settings" element={<OrgSettingsPage />} />
      <Route path="organizations/:org/settings/*" element={<OrgSettingsPage />} />
      <Route path="orgs/:org" element={<OrgRedirect />} />
      <Route path="organizations/:org" element={<OrgRedirect />} />
      <Route path="orgs/:org/repositories" element={<OrgRepositoriesPage />} />
      <Route path="orgs/:org/invitation" element={<InvitationPage />} />
      <Route path="orgs/:org/people" element={<PeoplePage view="members" />} />
      <Route path="orgs/:org/outside-collaborators" element={<PeoplePage view="outside" />} />
      <Route path="orgs/:org/people/pending_invitations" element={<PeoplePage view="pending" />} />
      <Route path="orgs/:org/people/failed_invitations" element={<PeoplePage view="failed" />} />
      <Route path="orgs/:org/people/security_managers" element={<PeoplePage view="security" />} />
      <Route path="orgs/:org/teams" element={<TeamsPage />} />
      <Route path="orgs/:org/new-team" element={<NewTeamPage />} />
      <Route path="orgs/:org/teams/:slug" element={<TeamPage tab="members" />} />
      <Route path="orgs/:org/teams/:slug/members" element={<TeamPage tab="members" />} />
      <Route path="orgs/:org/teams/:slug/teams" element={<TeamPage tab="teams" />} />
      <Route path="orgs/:org/teams/:slug/repositories" element={<TeamPage tab="repositories" />} />
      <Route path="orgs/:org/teams/:slug/settings" element={<TeamPage tab="settings" />} />
    </>
  ),
  orgProfile: OrgProfile,
  orgSettings: ownOrgSettings,
  userSettings: [
    { key: "organizations", path: "organizations", label: "Organizations", group: "Access", order: 450, element: OrganizationsSettings },
  ],
  userMenu: [
    { key: "your-orgs", label: "Your organizations", to: "/settings/organizations", order: 300 },
    { key: "new-org", label: "New organization", to: "/organizations/new", order: 310 },
  ],
  slots: {
    "home.sidebar": [PendingOrgInvites],
  },
};

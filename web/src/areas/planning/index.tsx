import { Navigate, Route, useParams } from "react-router-dom";
import type { User } from "../../api";
import { CommentIcon } from "../../components/icons";
import { ChooseIssuePage } from "../../pages/Issues";
import type { Area } from "../types";
import { Dashboard } from "./Dashboards";
import { DiscussionPage, DiscussionsPage, NewDiscussionPage } from "./Discussions";
import { LabelsPage } from "./Labels";
import { EditMilestonePage, MilestonePage, MilestonesPage, NewMilestonePage } from "./Milestones";
import { OrgProjectsPage, OwnerProjects, ProjectPage, ProjectSettingsPage, RepoProjectsPage, UserProjectsPage } from "./Projects";
import "./styles.css";

const ProjectsIcon = ({ size = 16 }: { size?: number; className?: string }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" className="icon">
    <path d="M1.75 0h12.5C15.216 0 16 .784 16 1.75v12.5A1.75 1.75 0 0 1 14.25 16H1.75A1.75 1.75 0 0 1 0 14.25V1.75C0 .784.784 0 1.75 0ZM1.5 1.75v12.5c0 .138.112.25.25.25h12.5a.25.25 0 0 0 .25-.25V1.75a.25.25 0 0 0-.25-.25H1.75a.25.25 0 0 0-.25.25ZM11.75 3a.75.75 0 0 1 .75.75v7.5a.75.75 0 0 1-1.5 0v-7.5a.75.75 0 0 1 .75-.75Zm-8.25.75a.75.75 0 0 1 1.5 0v5.5a.75.75 0 0 1-1.5 0ZM8 3a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 8 3Z" />
  </svg>
);

function ProfileProjects({ user }: { user: User }) {
  return <OwnerProjects login={user.login} kind="users" embedded />;
}

function ProjectRedirect({ kind }: { kind: "users" | "orgs" }) {
  const p = useParams();
  return <Navigate to={`/${kind}/${kind === "orgs" ? p.org : p.login}/projects/${p.number}/views/1`} replace />;
}

const projectRoutes = (kind: "users" | "orgs", param: string) => (
  <>
    <Route path={`${kind}/:${param}/projects`} element={kind === "users" ? <UserProjectsPage /> : <OrgProjectsPage />} />
    <Route path={`${kind}/:${param}/projects/:number`} element={<ProjectRedirect kind={kind} />} />
    <Route path={`${kind}/:${param}/projects/:number/views/:view`} element={<ProjectPage kind={kind} />} />
    <Route path={`${kind}/:${param}/projects/:number/settings`} element={<ProjectSettingsPage kind={kind} />} />
  </>
);

// The planning area: issues/PR extras, labels, milestones, dashboards, projects, discussions.
export const area: Area = {
  name: "planning",
  topRoutes: (
    <>
      <Route path="issues" element={<Dashboard kind="issues" />} />
      <Route path="pulls" element={<Dashboard kind="pulls" />} />
      {projectRoutes("users", "login")}
      {projectRoutes("orgs", "org")}
    </>
  ),
  repoRoutes: (
    <>
      <Route path="labels" element={<LabelsPage />} />
      <Route path="milestones" element={<MilestonesPage />} />
      <Route path="milestones/new" element={<NewMilestonePage />} />
      <Route path="milestones/:number/edit" element={<EditMilestonePage />} />
      <Route path="milestone/:number" element={<MilestonePage />} />
      <Route path="issues/new/choose" element={<ChooseIssuePage />} />
      <Route path="projects" element={<RepoProjectsPage />} />
      <Route path="discussions" element={<DiscussionsPage />} />
      <Route path="discussions/new" element={<NewDiscussionPage />} />
      <Route path="discussions/categories/:slug" element={<DiscussionsPage />} />
      <Route path="discussions/:number" element={<DiscussionPage />} />
    </>
  ),
  repoTabs: [
    {
      key: "discussions", label: "Discussions", icon: CommentIcon, path: "discussions", order: 30,
      show: (ctx) => !!(ctx.repo as { has_discussions?: boolean }).has_discussions,
    },
    {
      key: "projects", label: "Projects", icon: ProjectsIcon, path: "projects", order: 40,
      show: (ctx) => (ctx.repo as { has_projects?: boolean }).has_projects !== false,
    },
  ],
  headerNav: [
    { key: "issues", label: "Issues", to: "/issues", order: 10 },
    { key: "pulls", label: "Pull requests", to: "/pulls", order: 20 },
  ],
  userMenu: [
    { key: "projects", label: "Your projects", to: (me) => `/users/${me.login}/projects`, order: 20 },
  ],
  profileTabs: [
    { key: "projects", label: "Projects", order: 30, icon: ProjectsIcon, element: ProfileProjects, show: (u) => u.type !== "Organization" },
  ],
};

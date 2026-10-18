import { BrowserRouter, Navigate, Route, Routes, useParams } from "react-router-dom";
import { AuthProvider } from "./auth";
import { NotFoundView } from "./components/common";
import { Layout } from "./components/Layout";
import { RepoLayout } from "./components/RepoLayout";
import { LoginPage, SignupPage } from "./pages/Auth";
import { BlobPage, CodePage } from "./pages/Code";
import { CommitPage, CommitsPage } from "./pages/Commits";
import { HomePage } from "./pages/Home";
import { IssuePage, IssuesPage, NewIssuePage } from "./pages/Issues";
import { NewRepoPage } from "./pages/NewRepo";
import { ProfilePage } from "./pages/Profile";
import { ComparePage, PullPage, PullsPage } from "./pages/Pulls";
import { RepoSettingsPage } from "./pages/RepoSettings";
import { TokensPage } from "./pages/Tokens";
import { KeysPage } from "./pages/Keys";
import { BranchesPage, TagsPage } from "./pages/Refs";
import { NewReleasePage, ReleasePage, ReleasesPage } from "./pages/Releases";
import { SearchPage } from "./pages/Search";
import { UserSettingsPage } from "./pages/UserSettings";
import { repoRoutes, topRoutes } from "./areas";

/** github.com-style `/o/r/pulls/123` → `/o/r/pull/123`. */
function PullsRedirect() {
  const { owner, repo, number } = useParams();
  return <Navigate to={`/${owner}/${repo}/pull/${number}`} replace />;
}

export function App() {
  return (
    <BrowserRouter>
      <AuthProvider>
        <Routes>
          <Route element={<Layout />}>
            <Route index element={<HomePage />} />
            <Route path="login" element={<LoginPage />} />
            <Route path="signup" element={<SignupPage />} />
            <Route path="new" element={<NewRepoPage />} />
            <Route path="settings/tokens" element={<TokensPage />} />
            <Route path="settings/keys" element={<KeysPage />} />
            <Route path="search" element={<SearchPage />} />
            <Route path="settings" element={<UserSettingsPage />} />
            <Route path="settings/*" element={<UserSettingsPage />} />
            {/* one-segment sections (/settings/x) would otherwise tie with :owner/:repo and lose */}
            <Route path="settings/:section" element={<UserSettingsPage />} />
            {topRoutes()}
            <Route path=":owner" element={<ProfilePage />} />
            <Route path=":owner/:repo" element={<RepoLayout />}>
              <Route index element={<CodePage />} />
              <Route path="tree/*" element={<CodePage />} />
              <Route path="blob/*" element={<BlobPage />} />
              <Route path="commits" element={<CommitsPage />} />
              <Route path="commits/*" element={<CommitsPage />} />
              <Route path="commit/:sha" element={<CommitPage />} />
              <Route path="issues" element={<IssuesPage />} />
              <Route path="issues/new" element={<NewIssuePage />} />
              <Route path="issues/:number" element={<IssuePage />} />
              <Route path="pulls" element={<PullsPage />} />
              <Route path="pulls/new" element={<ComparePage />} />
              <Route path="pulls/:number" element={<PullsRedirect />} />
              <Route path="compare" element={<ComparePage />} />
              <Route path="compare/*" element={<ComparePage />} />
              <Route path="pull/:number" element={<PullPage tab="conversation" />} />
              <Route path="pull/:number/commits" element={<PullPage tab="commits" />} />
              <Route path="pull/:number/files" element={<PullPage tab="files" />} />
              <Route path="branches" element={<BranchesPage />} />
              <Route path="tags" element={<TagsPage />} />
              <Route path="releases" element={<ReleasesPage />} />
              <Route path="releases/new" element={<NewReleasePage />} />
              <Route path="releases/latest" element={<ReleasePage latest />} />
              <Route path="releases/tag/*" element={<ReleasePage />} />
              <Route path="settings" element={<RepoSettingsPage />} />
              <Route path="settings/*" element={<RepoSettingsPage />} />
              {repoRoutes()}
              <Route path="*" element={<NotFoundView />} />
            </Route>
            <Route path="*" element={<NotFoundView />} />
          </Route>
        </Routes>
      </AuthProvider>
    </BrowserRouter>
  );
}


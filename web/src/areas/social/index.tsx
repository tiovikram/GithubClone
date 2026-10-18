import { Route } from "react-router-dom";
import type { Area } from "../types";
import { ExplorePage } from "./explore";
import {
  DiscoverGists, EditGistPage, GistForksPage, GistPage, GistRevisionsPage, GistsHome, NewGistPage, StarredGists, UserGists,
} from "./gists";
import { NotificationBell, NotificationsPage, WatchingPage } from "./notifications";
import { ActivityTab, FollowCard, FollowersTab, FollowingTab } from "./profile";
import {
  AccountSettings, AppearanceSettings, BlockedUsersSettings, EmailSettings, PasswordSettings, ProfileSettings, SigningKeysSettings,
} from "./settings";
import { IssueSubscribe, PullSubscribe, StargazersPage, WatchButton, WatchersPage } from "./watch";
import { PeopleIcon, PulseIcon } from "./icons";
import "./styles.css";

// The social area: notifications, watching, follows, activity, gists, personal settings.
export const area: Area = {
  name: "social",
  topRoutes: (
    <>
      <Route path="notifications" element={<NotificationsPage />} />
      <Route path="watching" element={<WatchingPage />} />
      <Route path="explore" element={<ExplorePage />} />
      <Route path="gists" element={<GistsHome />} />
      <Route path="gists/new" element={<NewGistPage />} />
      <Route path="gists/discover" element={<DiscoverGists />} />
      <Route path="gists/starred" element={<StarredGists />} />
      <Route path="gist" element={<GistsHome />} />
      <Route path="gist/:login" element={<UserGists />} />
      <Route path="gist/:login/:id" element={<GistPage />} />
      <Route path="gist/:login/:id/edit" element={<EditGistPage />} />
      <Route path="gist/:login/:id/revisions" element={<GistRevisionsPage />} />
      <Route path="gist/:login/:id/forks" element={<GistForksPage />} />
      <Route path="gist/:login/:id/:sha" element={<GistPage />} />
    </>
  ),
  repoRoutes: (
    <>
      <Route path="stargazers" element={<StargazersPage />} />
      <Route path="watchers" element={<WatchersPage />} />
    </>
  ),
  headerNav: [
    { key: "explore", label: "Explore", to: "/explore", order: 80 },
  ],
  userMenu: [
    { key: "gists", label: "Your gists", to: (me) => `/gist/${me.login}`, order: 20 },
    { key: "followers", label: "Your followers", to: (me) => `/${me.login}?tab=followers`, order: 21 },
    { key: "notifications", label: "Notifications", to: "/notifications", order: 22 },
    { key: "watching", label: "Watched repositories", to: "/watching", order: 23 },
  ],
  profileTabs: [
    { key: "activity", label: "Activity", order: 5, icon: PulseIcon, element: ActivityTab, show: (u) => u.type === "User" },
    { key: "followers", label: "Followers", order: 90, icon: PeopleIcon, element: FollowersTab, show: (u) => u.type === "User" },
    { key: "following", label: "Following", order: 91, icon: PeopleIcon, element: FollowingTab, show: (u) => u.type === "User" },
  ],
  userSettings: [
    { key: "profile", path: "profile", label: "Public profile", order: 0, element: ProfileSettings },
    { key: "account", path: "admin", label: "Account", order: 10, element: AccountSettings },
    { key: "appearance", path: "appearance", label: "Appearance", order: 20, element: AppearanceSettings },
    { key: "emails", path: "emails", label: "Emails", group: "Access", order: 490, element: EmailSettings },
    { key: "security", path: "security", label: "Password and authentication", group: "Access", order: 495, element: PasswordSettings },
    { key: "gpg", path: "gpg_keys", label: "GPG and signing keys", group: "Access", order: 515, element: SigningKeysSettings },
    { key: "blocked", path: "blocked_users", label: "Blocked users", group: "Moderation", order: 700, element: BlockedUsersSettings },
  ],
  slots: {
    "header.right": [NotificationBell],
    "repo.header.actions": [WatchButton],
    "issue.sidebar": [IssueSubscribe],
    "pull.sidebar": [PullSubscribe],
    "profile.sidebar": [FollowCard],
  },
};

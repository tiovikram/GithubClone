// Profile extras: follow button + counts (sidebar), followers/following/activity tabs, block.
import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { errorMessage, type User } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, Pagination } from "../../components/common";
import { useAsync } from "../../hooks";
import { sapi } from "./api";
import { EventFeed } from "./feed";
import { BlockIcon, GistIcon, LinkIcon, PeopleIcon } from "./icons";

export function FollowCard({ user, me }: { user: User; me: User | null }) {
  const nav = useNavigate();
  const isUser = user.type === "User";
  const self = me?.login.toLowerCase() === user.login.toLowerCase();
  const st = useAsync(async () => {
    const [following, blocked] = me && !self && isUser ? await Promise.all([sapi.isFollowing(user.login), sapi.isBlocked(user.login)]) : [false, false];
    return { following, blocked };
  }, [me?.login, user.login]);
  const counts = useAsync(() => sapi.user(user.login), [user.login, st.data?.following, st.data?.blocked]);
  const social = useAsync(() => sapi.userSocialAccounts(user.login).catch(() => []), [user.login]);
  const [busy, setBusy] = useState(false);
  if (!isUser) return null;
  const followers = counts.data?.followers ?? user.followers ?? 0;
  const following = counts.data?.following ?? user.following ?? 0;
  const run = async (fn: () => Promise<unknown>, next: Partial<{ following: boolean; blocked: boolean }>) => {
    setBusy(true);
    try {
      await fn();
      st.setData({ following: false, blocked: false, ...st.data, ...next });
    } catch (e) {
      alert(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const tw = counts.data?.twitter_username;
  return (
    <div className="follow-card" data-testid="follow-card">
      {!self && (
        me ? (
          st.data && !st.data.blocked && (
            <button type="button" className="btn btn-block" disabled={busy} data-testid="follow-button"
              onClick={() => (st.data!.following ? run(() => sapi.unfollow(user.login), { following: false }) : run(() => sapi.follow(user.login), { following: true }))}>
              {st.data.following ? "Unfollow" : "Follow"}
            </button>
          )
        ) : (
          <button type="button" className="btn btn-block" onClick={() => nav(`/login?return_to=${encodeURIComponent(`/${user.login}`)}`)}>Follow</button>
        )
      )}
      <div className="follow-counts small">
        <PeopleIcon />{" "}
        <Link to={`/${user.login}?tab=followers`} data-testid="followers-link"><strong data-testid="followers-count">{followers}</strong> <span className="muted">follower{followers === 1 ? "" : "s"}</span></Link>
        {" · "}
        <Link to={`/${user.login}?tab=following`} data-testid="following-link"><strong data-testid="following-count">{following}</strong> <span className="muted">following</span></Link>
      </div>
      <ul className="profile-details muted small">
        {counts.data?.hireable && <li data-testid="hireable">Available for hire</li>}
        {tw && <li><LinkIcon /> <a href={`https://twitter.com/${tw}`} rel="nofollow noopener">@{tw}</a></li>}
        {(social.data || []).map((a) => (
          <li key={a.url} data-testid="social-account"><LinkIcon /> <a href={a.url} rel="nofollow noopener me">{a.url.replace(/^https?:\/\//, "")}</a></li>
        ))}
        <li><GistIcon /> <Link to={`/gist/${user.login}`} data-testid="profile-gists-link">Gists</Link></li>
      </ul>
      {me && !self && st.data && (
        <button type="button" className="btn-link small muted block-link" disabled={busy} data-testid="block-button"
          onClick={() => {
            if (st.data!.blocked) void run(() => sapi.unblock(user.login), { blocked: false });
            else if (confirm(`Block ${user.login}? They won't be able to follow you or interact with your repositories.`)) void run(() => sapi.block(user.login), { blocked: true, following: false });
          }}>
          <BlockIcon /> {st.data.blocked ? "Unblock user" : "Block user"}
        </button>
      )}
      {st.data?.blocked && <p className="small muted" data-testid="blocked-note">You have blocked this user.</p>}
    </div>
  );
}

function PeopleList({ load, user, kind }: { load: (u: string, p: number) => ReturnType<typeof sapi.followers>; user: User; kind: "followers" | "following" }) {
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  const st = useAsync(() => load(user.login, page), [user.login, page, kind]);
  return (
    <>
      <Async state={st}>
        {(p) => p.items.length === 0 ? (
          <Blankslate title={kind === "followers" ? `${user.login} doesn't have any followers yet.` : `${user.login} isn't following anybody.`} />
        ) : (
          <ul className="list people-list" data-testid={`${kind}-list`}>
            {p.items.map((u) => (
              <li key={u.id} className="list-row people-row">
                <Avatar user={u} size={48} />
                <div>
                  <Link to={`/${u.login}`} className="people-login"><strong>{u.login}</strong></Link>
                  {u.name && <div className="muted small">{u.name}</div>}
                </div>
              </li>
            ))}
          </ul>
        )}
      </Async>
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last} onPage={(n) => setSp({ tab: kind, page: String(n) })} />}
    </>
  );
}

export const FollowersTab = ({ user }: { user: User }) => <PeopleList load={sapi.followers} user={user} kind="followers" />;
export const FollowingTab = ({ user }: { user: User }) => <PeopleList load={sapi.following} user={user} kind="following" />;

export function ActivityTab({ user }: { user: User }) {
  const { user: me } = useAuth();
  return (
    <div>
      <h2 className="h4">Public activity</h2>
      <EventFeed load={(p) => sapi.userEvents(user.login, p)} loadKey={`${user.login}:${me?.login || ""}`} testid="profile-activity"
        empty={<Blankslate title={`${user.login} has no recent public activity.`} />} />
    </div>
  );
}

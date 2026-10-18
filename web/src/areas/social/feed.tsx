// Activity feed: renders GitHub events (/users/{u}/received_events, /users/{u}/events).
import { useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { Avatar, Blankslate, ErrorBox, RelTime, Spinner } from "../../components/common";
import { BranchIcon, CommentIcon, ForkIcon, IssueOpenIcon, PullIcon, RepoIcon, StarIcon, TagIcon } from "../../components/icons";
import { useAsync } from "../../hooks";
import type { Event } from "./api";
import type { Page } from "../../api";

const RepoA = ({ name }: { name: string }) => <Link to={`/${name}`} className="feed-repo">{name}</Link>;

function issuePath(repo: string, n: number, pull: boolean) {
  return `/${repo}/${pull ? "pull" : "issues"}/${n}`;
}

/** One sentence describing an event, plus an icon. */
export function describe(e: Event): { icon: ReactNode; text: ReactNode; body?: string } | null {
  const repo = e.repo?.name || "";
  const p = e.payload || {};
  const R = <RepoA name={repo} />;
  switch (e.type) {
    case "PushEvent": {
      const ref = String(p.ref || "").replace(/^refs\/heads\//, "");
      const n = p.size ?? p.commits?.length ?? 0;
      return {
        icon: <BranchIcon />,
        text: <>pushed {n ? `${n} commit${n === 1 ? "" : "s"} ` : ""}to <Link to={`/${repo}/tree/${ref}`}>{ref}</Link> in {R}</>,
        body: (p.commits || []).slice(0, 3).map((c: { message: string }) => c.message.split("\n")[0]).join("\n") || undefined,
      };
    }
    case "CreateEvent":
      if (p.ref_type === "repository") return { icon: <RepoIcon />, text: <>created a repository {R}</> };
      return { icon: p.ref_type === "tag" ? <TagIcon /> : <BranchIcon />, text: <>created {p.ref_type} <code>{p.ref}</code> in {R}</> };
    case "DeleteEvent":
      return { icon: <BranchIcon />, text: <>deleted {p.ref_type} <code>{p.ref}</code> in {R}</> };
    case "WatchEvent":
      return { icon: <StarIcon className="icon c-star" />, text: <>starred {R}</> };
    case "ForkEvent":
      return { icon: <ForkIcon />, text: <>forked {R}{p.forkee?.full_name && <> to <RepoA name={p.forkee.full_name} /></>}</> };
    case "IssuesEvent": {
      const i = p.issue || {};
      return {
        icon: <IssueOpenIcon />,
        text: <>{p.action} issue <Link to={issuePath(repo, i.number, false)}>{i.title} #{i.number}</Link> in {R}</>,
      };
    }
    case "IssueCommentEvent": {
      const i = p.issue || {};
      return {
        icon: <CommentIcon />,
        text: <>commented on {i.pull_request ? "pull request" : "issue"} <Link to={issuePath(repo, i.number, !!i.pull_request)}>{i.title} #{i.number}</Link> in {R}</>,
        body: p.comment?.body,
      };
    }
    case "PullRequestEvent": {
      const pr = p.pull_request || {};
      const action = p.action === "closed" && pr.merged ? "merged" : p.action;
      return { icon: <PullIcon />, text: <>{action} pull request <Link to={issuePath(repo, pr.number, true)}>{pr.title} #{pr.number}</Link> in {R}</> };
    }
    case "PullRequestReviewEvent": {
      const pr = p.pull_request || {};
      return { icon: <PullIcon />, text: <>reviewed pull request <Link to={issuePath(repo, pr.number, true)}>{pr.title} #{pr.number}</Link> in {R}</> };
    }
    case "PullRequestReviewCommentEvent": {
      const pr = p.pull_request || {};
      return { icon: <CommentIcon />, text: <>commented on pull request <Link to={issuePath(repo, pr.number, true)}>{pr.title} #{pr.number}</Link> in {R}</>, body: p.comment?.body };
    }
    case "ReleaseEvent": {
      const r = p.release || {};
      return { icon: <TagIcon />, text: <>{p.action} release <Link to={`/${repo}/releases/tag/${r.tag_name}`}>{r.name || r.tag_name}</Link> in {R}</> };
    }
    case "CommitCommentEvent":
      return { icon: <CommentIcon />, text: <>commented on a commit in {R}</>, body: p.comment?.body };
    case "MemberEvent":
      return { icon: <RepoIcon />, text: <>{p.action} <Link to={`/${p.member?.login}`}>{p.member?.login}</Link> as a collaborator to {R}</> };
    case "PublicEvent":
      return { icon: <RepoIcon />, text: <>made {R} public</> };
    default:
      return repo ? { icon: <RepoIcon />, text: <>{e.type.replace(/Event$/, "").toLowerCase()} in {R}</> } : null;
  }
}

export function EventRow({ e }: { e: Event }) {
  const d = describe(e);
  if (!d) return null;
  return (
    <li className="feed-item" data-testid="feed-item" data-type={e.type}>
      <span className="feed-icon muted">{d.icon}</span>
      <div className="feed-main">
        <div>
          <Link to={`/${e.actor.login}`} className="feed-actor"><Avatar user={e.actor} size={20} /> <strong>{e.actor.display_login || e.actor.login}</strong></Link>{" "}
          {d.text} <span className="muted small"><RelTime iso={e.created_at} /></span>
        </div>
        {d.body && <div className="feed-body muted small">{d.body.length > 200 ? `${d.body.slice(0, 200)}…` : d.body}</div>}
      </div>
    </li>
  );
}

/** Paged event list with "More" button. */
export function EventFeed({ load, loadKey, empty, testid }: { load: (page: number) => Promise<Page<Event[]>>; loadKey: string; empty: ReactNode; testid?: string }) {
  const [pages, setPages] = useState(1);
  const st = useAsync(async () => {
    const out: Event[] = [];
    let next: number | undefined = undefined;
    for (let p = 1; p <= pages; p++) {
      const r = await load(p);
      out.push(...r.items);
      next = r.next;
      if (!r.next) break;
    }
    return { items: out, more: !!next };
  }, [pages, loadKey]);
  if (st.error) return <ErrorBox error={st.error} onRetry={st.reload} />;
  if (!st.data) return <Spinner />;
  const items = st.data.items.filter((e) => describe(e));
  if (items.length === 0) return <>{empty}</>;
  return (
    <div data-testid={testid}>
      <ul className="feed">
        {items.map((e) => <EventRow key={e.id} e={e} />)}
      </ul>
      {st.data.more && (
        <button type="button" className="btn btn-block" onClick={() => setPages(pages + 1)} disabled={st.loading}>
          {st.loading ? "Loading…" : "More"}
        </button>
      )}
    </div>
  );
}

export const NoActivity = ({ title, children }: { title: string; children?: ReactNode }) => <Blankslate title={title}>{children}</Blankslate>;

// Timeline events of issues and pull requests (from GET /issues/{n}/timeline), rendered the way
// github.com words them.
import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { LabelPill, RelTime, UserLink } from "../../components/common";
import {
  BranchIcon, CommentIcon, CommitIcon, IssueClosedIcon, IssueOpenIcon, LockIcon, PullIcon, TagIcon,
} from "../../components/icons";
import type { TimelineEvent } from "./api";

/** Events rendered elsewhere (comments, reviews, merges) or hidden like on github.com. */
const SKIP = new Set(["commented", "reviewed", "line-commented", "commit-commented", "subscribed", "unsubscribed", "mentioned", "merged"]);

export function shownEvents(events: TimelineEvent[]): TimelineEvent[] {
  return events.filter((e) => !SKIP.has(e.event));
}

export function eventTime(e: TimelineEvent): string {
  return e.created_at || e.submitted_at || e.author?.date || "";
}

const reasonText = (r?: string | null) => (r ? r.replace(/_/g, " ") : "");

export function TimelineEventRow({ e, base }: { e: TimelineEvent; base: string }) {
  const actor = e.actor || e.user || null;
  const who = <UserLink user={actor} />;
  const when = <RelTime iso={eventTime(e)} />;
  let icon: ReactNode = <CommentIcon />;
  let text: ReactNode;
  switch (e.event) {
    case "labeled":
    case "unlabeled":
      icon = <TagIcon />;
      text = <>{who} {e.event === "labeled" ? "added" : "removed"} {e.label && <LabelPill label={{ name: e.label.name, color: e.label.color, description: null }} />} label {when}</>;
      break;
    case "assigned":
    case "unassigned": {
      const self = e.assignee && actor && e.assignee.login === actor.login;
      text = e.event === "assigned"
        ? (self ? <>{who} self-assigned this {when}</> : <>{who} assigned <UserLink user={e.assignee} /> {when}</>)
        : (self ? <>{who} removed their assignment {when}</> : <>{who} unassigned <UserLink user={e.assignee} /> {when}</>);
      break;
    }
    case "milestoned":
      text = <>{who} added this to the <strong>{e.milestone?.title}</strong> milestone {when}</>;
      break;
    case "demilestoned":
      text = <>{who} removed this from the <strong>{e.milestone?.title}</strong> milestone {when}</>;
      break;
    case "renamed":
      text = <>{who} changed the title <del>{e.rename?.from}</del> <ins className="pl-ins">{e.rename?.to}</ins> {when}</>;
      break;
    case "closed":
      icon = <IssueClosedIcon className="icon c-done" />;
      text = <>{who} closed this{e.state_reason && e.state_reason !== "completed" ? ` as ${reasonText(e.state_reason)}` : e.state_reason ? " as completed" : ""}
        {e.commit_id ? <> in <Link className="mono" to={`${base}/commit/${e.commit_id}`}>{e.commit_id.slice(0, 7)}</Link></> : null} {when}</>;
      break;
    case "reopened":
      icon = <IssueOpenIcon className="icon c-open" />;
      text = <>{who} reopened this {when}</>;
      break;
    case "locked":
      icon = <LockIcon />;
      text = <>{who} locked{e.lock_reason ? <> as <strong>{e.lock_reason}</strong></> : null} and limited conversation to collaborators {when}</>;
      break;
    case "unlocked":
      icon = <LockIcon />;
      text = <>{who} unlocked this conversation {when}</>;
      break;
    case "referenced":
      icon = <CommitIcon />;
      text = <>{who} added a commit that referenced this issue {e.commit_id && <Link className="mono" to={`${base}/commit/${e.commit_id}`}>{e.commit_id.slice(0, 7)}</Link>} {when}</>;
      break;
    case "cross-referenced": {
      const src = e.source?.issue;
      const repoName = src?.repository?.full_name || (src?.repository_url ? src.repository_url.split("/repos/")[1] : "");
      const kind = src?.pull_request ? "pull" : "issues";
      text = <>{who} mentioned this {when}{src && (
        <div className="pl-xref">
          {src.pull_request ? <PullIcon /> : <IssueOpenIcon />}{" "}
          <Link to={`/${repoName}/${kind}/${src.number}`}>{src.title}</Link> <span className="muted">{repoName}#{src.number}</span>
        </div>
      )}</>;
      break;
    }
    case "review_requested":
      text = <>{who} requested a review from <UserLink user={e.requested_reviewer} /> {when}</>;
      break;
    case "review_request_removed":
      text = <>{who} removed the request for review from <UserLink user={e.requested_reviewer} /> {when}</>;
      break;
    case "review_dismissed":
      text = <>{who} dismissed a review{e.dismissed_review?.dismissal_message ? <>: <em>{e.dismissed_review.dismissal_message}</em></> : null} {when}</>;
      break;
    case "ready_for_review":
      icon = <PullIcon />;
      text = <>{who} marked this pull request as ready for review {when}</>;
      break;
    case "convert_to_draft":
      icon = <PullIcon />;
      text = <>{who} marked this pull request as draft {when}</>;
      break;
    case "base_ref_changed":
      icon = <BranchIcon />;
      text = <>{who} changed the base branch {when}</>;
      break;
    case "head_ref_force_pushed":
      icon = <BranchIcon />;
      text = <>{who} force-pushed the branch {when}</>;
      break;
    case "head_ref_deleted":
      icon = <BranchIcon />;
      text = <>{who} deleted the head branch {when}</>;
      break;
    case "head_ref_restored":
      icon = <BranchIcon />;
      text = <>{who} restored the head branch {when}</>;
      break;
    case "pinned":
      text = <>{who} pinned this issue {when}</>;
      break;
    case "unpinned":
      text = <>{who} unpinned this issue {when}</>;
      break;
    case "transferred":
      text = <>{who} transferred this issue {when}</>;
      break;
    case "connected":
      text = <>{who} linked a pull request that will close this issue {when}</>;
      break;
    case "committed":
      icon = <CommitIcon />;
      text = <><span className="pl-commit-msg">{(e.message || "").split("\n")[0]}</span>{" "}
        {e.sha && <Link className="mono muted-link" to={`${base}/commit/${e.sha}`}>{e.sha.slice(0, 7)}</Link>}</>;
      break;
    case "deployed":
      text = <>{who} deployed this {when}</>;
      break;
    default:
      text = <>{who} {e.event.replace(/_/g, " ")} {when}</>;
  }
  return (
    <div className="timeline-event pl-event" data-event={e.event} data-testid="timeline-event">
      <span className="pl-event-icon">{icon}</span>
      <span className="pl-event-text">{text}</span>
    </div>
  );
}

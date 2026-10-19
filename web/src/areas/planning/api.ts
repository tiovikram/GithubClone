// Endpoint helpers for the planning area (issues/PR extras, milestones, labels, projects, discussions).
import {
  ApiError, request, requestAll, requestPage, type Issue, type IssueComment, type Label, type Pull, type Repo,
  type Review, type SearchResult, type User,
} from "../../api";

const enc = encodeURIComponent;
const rp = (o: string, r: string) => `/repos/${enc(o)}/${enc(r)}`;

// ---------- types ----------

export interface Reactions {
  "+1": number; "-1": number; laugh: number; confused: number; heart: number; hooray: number; rocket: number; eyes: number;
  total_count: number;
}
export type ReactionContent = "+1" | "-1" | "laugh" | "confused" | "heart" | "hooray" | "rocket" | "eyes";
export interface Reaction { id: number; content: ReactionContent; user: User | null; created_at: string }

export interface Milestone {
  id: number;
  number: number;
  title: string;
  description: string | null;
  state: "open" | "closed";
  open_issues: number;
  closed_issues: number;
  due_on: string | null;
  closed_at: string | null;
  created_at: string;
  updated_at: string;
  creator?: User | null;
}

export type FullIssue = Issue & {
  node_id?: string;
  milestone?: Milestone | null;
  reactions?: Reactions;
  active_lock_reason?: string | null;
  assignee?: User | null;
};

export type FullComment = IssueComment & { node_id?: string; reactions?: Reactions; pin?: { pinned_at: string; pinned_by: User | null } | null };

export type FullPull = Pull & {
  node_id?: string;
  milestone?: Milestone | null;
  assignees?: User[];
  requested_reviewers?: User[];
  requested_teams?: { slug: string; name: string }[];
  auto_merge?: { merge_method: string; enabled_by: User | null; commit_title?: string | null } | null;
  locked?: boolean;
  active_lock_reason?: string | null;
  maintainer_can_modify?: boolean;
  rebaseable?: boolean | null;
};

export type FullReview = Review & { commit_id?: string; node_id?: string };

export interface ReviewComment {
  id: number;
  node_id?: string;
  pull_request_review_id: number | null;
  in_reply_to_id?: number;
  user: User | null;
  body: string;
  path: string;
  diff_hunk: string;
  line: number | null;
  original_line: number | null;
  start_line: number | null;
  original_start_line?: number | null;
  side: "LEFT" | "RIGHT";
  start_side?: "LEFT" | "RIGHT" | null;
  position: number | null;
  commit_id: string;
  original_commit_id: string;
  created_at: string;
  updated_at: string;
  html_url: string;
  author_association?: string;
  reactions?: Reactions;
  subject_type?: string;
}

export interface TimelineEvent {
  id?: number;
  node_id?: string;
  event: string;
  actor?: User | null;
  user?: User | null;
  created_at?: string;
  submitted_at?: string;
  commit_id?: string | null;
  label?: { name: string; color: string };
  assignee?: User;
  assigner?: User;
  requested_reviewer?: User;
  review_requester?: User;
  rename?: { from: string; to: string };
  milestone?: { title: string };
  lock_reason?: string | null;
  state_reason?: string | null;
  dismissed_review?: { state: string; review_id: number; dismissal_message: string | null };
  source?: { type: string; issue?: Issue & { repository?: Repo } };
  // committed events
  sha?: string;
  message?: string;
  author?: { name: string; date: string };
  // commented
  body?: string;
  state?: string;
  html_url?: string;
}

export interface NewReviewComment {
  path: string;
  body: string;
  line: number;
  side: "LEFT" | "RIGHT";
  start_line?: number;
  start_side?: "LEFT" | "RIGHT";
}

// ---------- GraphQL ----------

export interface GqlError { message: string; type?: string; path?: string[] }

export async function gql<T>(query: string, variables: Record<string, unknown> = {}): Promise<T> {
  const res = await request<{ data?: T; errors?: GqlError[] }>("/api/graphql", { method: "POST", body: { query, variables } });
  if (res.errors?.length) {
    const nf = res.errors.find((e) => e.type === "NOT_FOUND");
    throw new ApiError(nf ? 404 : 422, { message: res.errors.map((e) => e.message).join("; ") }, res.errors[0].message);
  }
  return res.data as T;
}

// ---------- REST ----------

export const papi = {
  // labels
  labels: (o: string, r: string) => requestAll<Label>(`${rp(o, r)}/labels`, {}, 5),
  createLabel: (o: string, r: string, b: { name: string; color: string; description?: string }) =>
    request<Label>(`${rp(o, r)}/labels`, { method: "POST", body: b }),
  updateLabel: (o: string, r: string, name: string, b: { new_name?: string; color?: string; description?: string }) =>
    request<Label>(`${rp(o, r)}/labels/${enc(name)}`, { method: "PATCH", body: b }),
  deleteLabel: (o: string, r: string, name: string) => request<void>(`${rp(o, r)}/labels/${enc(name)}`, { method: "DELETE" }),

  // milestones
  milestones: (o: string, r: string, state: string, sort = "due_on", direction = "asc") =>
    requestAll<Milestone>(`${rp(o, r)}/milestones`, { query: { state, sort, direction } }, 5),
  milestone: (o: string, r: string, n: number) => request<Milestone>(`${rp(o, r)}/milestones/${n}`),
  createMilestone: (o: string, r: string, b: { title: string; description?: string; due_on?: string | null; state?: string }) =>
    request<Milestone>(`${rp(o, r)}/milestones`, { method: "POST", body: b }),
  updateMilestone: (o: string, r: string, n: number, b: { title?: string; description?: string; due_on?: string | null; state?: string }) =>
    request<Milestone>(`${rp(o, r)}/milestones/${n}`, { method: "PATCH", body: b }),
  deleteMilestone: (o: string, r: string, n: number) => request<void>(`${rp(o, r)}/milestones/${n}`, { method: "DELETE" }),

  // issue list with filters
  issues: (o: string, r: string, q: Record<string, string | number | undefined>) =>
    requestPage<FullIssue[]>(`${rp(o, r)}/issues`, { query: { per_page: 25, ...q } }),
  issue: (o: string, r: string, n: number) => request<FullIssue>(`${rp(o, r)}/issues/${n}`),
  createIssue: (o: string, r: string, b: { title: string; body?: string; labels?: string[]; assignees?: string[]; milestone?: number | null }) =>
    request<FullIssue>(`${rp(o, r)}/issues`, { method: "POST", body: b }),
  updateIssue: (o: string, r: string, n: number, b: Record<string, unknown>) =>
    request<FullIssue>(`${rp(o, r)}/issues/${n}`, { method: "PATCH", body: b }),
  assignees: (o: string, r: string) => requestAll<User>(`${rp(o, r)}/assignees`, {}, 3),
  addAssignees: (o: string, r: string, n: number, assignees: string[]) =>
    request<FullIssue>(`${rp(o, r)}/issues/${n}/assignees`, { method: "POST", body: { assignees } }),
  removeAssignees: (o: string, r: string, n: number, assignees: string[]) =>
    request<FullIssue>(`${rp(o, r)}/issues/${n}/assignees`, { method: "DELETE", body: { assignees } }),
  lock: (o: string, r: string, n: number, lock_reason?: string) =>
    request<void>(`${rp(o, r)}/issues/${n}/lock`, { method: "PUT", body: lock_reason ? { lock_reason } : {} }),
  unlock: (o: string, r: string, n: number) => request<void>(`${rp(o, r)}/issues/${n}/lock`, { method: "DELETE" }),
  timeline: (o: string, r: string, n: number) => requestAll<TimelineEvent>(`${rp(o, r)}/issues/${n}/timeline`, {}, 10),

  // comments
  comments: (o: string, r: string, n: number) => requestAll<FullComment>(`${rp(o, r)}/issues/${n}/comments`, {}, 10),
  addComment: (o: string, r: string, n: number, body: string) =>
    request<FullComment>(`${rp(o, r)}/issues/${n}/comments`, { method: "POST", body: { body } }),
  updateComment: (o: string, r: string, id: number, body: string) =>
    request<FullComment>(`${rp(o, r)}/issues/comments/${id}`, { method: "PATCH", body: { body } }),
  deleteComment: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/issues/comments/${id}`, { method: "DELETE" }),
  pinComment: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/issues/comments/${id}/pin`, { method: "PUT" }),
  unpinComment: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/issues/comments/${id}/pin`, { method: "DELETE" }),

  // reactions: subject path is relative to the repo, e.g. "issues/3", "issues/comments/9", "pulls/comments/4"
  reactions: (o: string, r: string, subject: string) => requestAll<Reaction>(`${rp(o, r)}/${subject}/reactions`, {}, 3),
  react: (o: string, r: string, subject: string, content: ReactionContent) =>
    request<Reaction>(`${rp(o, r)}/${subject}/reactions`, { method: "POST", body: { content } }),
  unreact: (o: string, r: string, subject: string, id: number) =>
    request<void>(`${rp(o, r)}/${subject}/reactions/${id}`, { method: "DELETE" }),

  // dependencies
  blockedBy: (o: string, r: string, n: number) => requestAll<FullIssue>(`${rp(o, r)}/issues/${n}/dependencies/blocked_by`, {}, 3),
  blocking: (o: string, r: string, n: number) => requestAll<FullIssue>(`${rp(o, r)}/issues/${n}/dependencies/blocking`, {}, 3),
  addBlockedBy: (o: string, r: string, n: number, issue_id: number) =>
    request<FullIssue>(`${rp(o, r)}/issues/${n}/dependencies/blocked_by`, { method: "POST", body: { issue_id } }),
  removeBlockedBy: (o: string, r: string, n: number, issue_id: number) =>
    request<FullIssue>(`${rp(o, r)}/issues/${n}/dependencies/blocked_by/${issue_id}`, { method: "DELETE" }),

  // sub-issues
  subIssues: (o: string, r: string, n: number) => requestAll<FullIssue>(`${rp(o, r)}/issues/${n}/sub_issues`, {}, 3),
  parentIssue: (o: string, r: string, n: number) => request<FullIssue>(`${rp(o, r)}/issues/${n}/parent`),
  addSubIssue: (o: string, r: string, n: number, sub_issue_id: number, replace_parent = false) =>
    request<FullIssue>(`${rp(o, r)}/issues/${n}/sub_issues`, { method: "POST", body: { sub_issue_id, replace_parent } }),
  removeSubIssue: (o: string, r: string, n: number, sub_issue_id: number) =>
    request<FullIssue>(`${rp(o, r)}/issues/${n}/sub_issue`, { method: "DELETE", body: { sub_issue_id } }),

  // pulls
  pull: (o: string, r: string, n: number) => request<FullPull>(`${rp(o, r)}/pulls/${n}`),
  updatePull: (o: string, r: string, n: number, b: Record<string, unknown>) =>
    request<FullPull>(`${rp(o, r)}/pulls/${n}`, { method: "PATCH", body: b }),
  requestReviewers: (o: string, r: string, n: number, reviewers: string[]) =>
    request<FullPull>(`${rp(o, r)}/pulls/${n}/requested_reviewers`, { method: "POST", body: { reviewers } }),
  removeReviewers: (o: string, r: string, n: number, reviewers: string[]) =>
    request<FullPull>(`${rp(o, r)}/pulls/${n}/requested_reviewers`, { method: "DELETE", body: { reviewers } }),
  updateBranch: (o: string, r: string, n: number) =>
    request<{ message: string }>(`${rp(o, r)}/pulls/${n}/update-branch`, { method: "PUT", body: {} }),
  reviews: (o: string, r: string, n: number) => requestAll<FullReview>(`${rp(o, r)}/pulls/${n}/reviews`, {}, 5),
  createReview: (o: string, r: string, n: number, b: { event?: string; body?: string; comments?: NewReviewComment[]; commit_id?: string }) =>
    request<FullReview>(`${rp(o, r)}/pulls/${n}/reviews`, { method: "POST", body: b }),
  dismissReview: (o: string, r: string, n: number, id: number, message: string) =>
    request<FullReview>(`${rp(o, r)}/pulls/${n}/reviews/${id}/dismissals`, { method: "PUT", body: { message, event: "DISMISS" } }),
  reviewComments: (o: string, r: string, n: number) => requestAll<ReviewComment>(`${rp(o, r)}/pulls/${n}/comments`, {}, 10),
  addReviewComment: (o: string, r: string, n: number, b: NewReviewComment & { commit_id: string }) =>
    request<ReviewComment>(`${rp(o, r)}/pulls/${n}/comments`, { method: "POST", body: b }),
  replyReviewComment: (o: string, r: string, n: number, id: number, body: string) =>
    request<ReviewComment>(`${rp(o, r)}/pulls/${n}/comments/${id}/replies`, { method: "POST", body: { body } }),
  updateReviewComment: (o: string, r: string, id: number, body: string) =>
    request<ReviewComment>(`${rp(o, r)}/pulls/comments/${id}`, { method: "PATCH", body: { body } }),
  deleteReviewComment: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/pulls/comments/${id}`, { method: "DELETE" }),

  // contents (apply suggestion)
  file: (o: string, r: string, path: string, ref: string) =>
    request<{ sha: string; content: string; encoding: string }>(`${rp(o, r)}/contents/${path.split("/").map(enc).join("/")}`, { query: { ref } }),
  putFile: (o: string, r: string, path: string, b: { message: string; content: string; sha: string; branch: string }) =>
    request<unknown>(`${rp(o, r)}/contents/${path.split("/").map(enc).join("/")}`, { method: "PUT", body: b }),

  // search (dashboards)
  search: (q: string, page = 1, sort = "updated") =>
    requestPage<SearchResult<FullIssue>>("/search/issues", { query: { q, sort, order: "desc", per_page: 25, page } }),

  myOrgs: () => requestAll<User>("/user/orgs", {}, 3),
  repoGet: (o: string, r: string) => request<Repo>(rp(o, r)),
};

export function b64encode(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

export type { Issue, Pull, Label, User, Repo };

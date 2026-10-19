// Pull request review threads (line comments + replies): grouping REST review comments into
// threads, their resolution state (GraphQL reviewThreads), and the thread UI shared by the
// Conversation and Files changed tabs.
import { useState } from "react";
import { errorMessage } from "../../api";
import { useAuth } from "../../auth";
import { Avatar, RelTime, UserLink } from "../../components/common";
import { MarkdownEditor } from "../../components/Conversation";
import { useAsync } from "../../hooks";
import { gql, papi, type FullPull, type ReviewComment } from "./api";
import { applySuggestion, CommentBody, hunkLines } from "./Suggestions";
import { confirmAction, KebabMenu, ReactionBar } from "./ui";

export interface Thread {
  root: ReviewComment;
  comments: ReviewComment[];
  /** GraphQL thread id and resolution. */
  node?: ThreadNode;
}
export interface ThreadNode {
  id: string;
  isResolved: boolean;
  viewerCanResolve: boolean;
  viewerCanUnresolve: boolean;
  resolvedBy: { login: string } | null;
  rootId: number;
}

export function groupThreads(comments: ReviewComment[], nodes: ThreadNode[] = []): Thread[] {
  const byId = new Map(comments.map((c) => [c.id, c]));
  const rootOf = (c: ReviewComment): ReviewComment => {
    let cur = c;
    for (let i = 0; i < 64 && cur.in_reply_to_id && byId.has(cur.in_reply_to_id); i++) cur = byId.get(cur.in_reply_to_id)!;
    return cur;
  };
  const threads = new Map<number, Thread>();
  for (const c of comments) {
    const r = rootOf(c);
    if (!threads.has(r.id)) threads.set(r.id, { root: r, comments: [], node: nodes.find((n) => n.rootId === r.id) });
    threads.get(r.id)!.comments.push(c);
  }
  for (const t of threads.values()) t.comments.sort((a, b) => a.id - b.id);
  return [...threads.values()].sort((a, b) => a.root.id - b.root.id);
}

/** Review comments + thread resolution for a pull request. */
export function useThreads(owner: string, name: string, n: number) {
  return useAsync(async () => {
    const [comments, d] = await Promise.all([
      papi.reviewComments(owner, name, n),
      gql<{ repository: { pullRequest: { reviewThreads: { nodes: { id: string; isResolved: boolean; viewerCanResolve: boolean; viewerCanUnresolve: boolean; resolvedBy: { login: string } | null; comments: { nodes: { databaseId: number }[] } }[] } } | null } | null }>(
        `query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r,followRenames:true){pullRequest(number:$n){reviewThreads(first:100){nodes{id isResolved viewerCanResolve viewerCanUnresolve resolvedBy{login} comments(first:1){nodes{databaseId}}}}}}}`,
        { o: owner, r: name, n },
      ).catch(() => null),
    ]);
    const nodes: ThreadNode[] = (d?.repository?.pullRequest?.reviewThreads.nodes || []).map((t) => ({
      id: t.id, isResolved: t.isResolved, viewerCanResolve: t.viewerCanResolve, viewerCanUnresolve: t.viewerCanUnresolve,
      resolvedBy: t.resolvedBy, rootId: t.comments.nodes[0]?.databaseId ?? -1,
    }));
    return { comments, threads: groupThreads(comments, nodes) };
  }, [owner, name, n]);
}

function HunkPreview({ c }: { c: ReviewComment }) {
  const lines = c.diff_hunk.split("\n");
  const tail = lines.slice(Math.max(1, lines.length - 4));
  return (
    <table className="diff-table pl-hunk">
      <tbody>
        {tail.map((l, i) => (
          <tr key={i} className={l.startsWith("+") ? "d-add" : l.startsWith("-") ? "d-del" : "d-ctx"}>
            <td className="code"><span className="d-marker" aria-hidden="true">{l[0] === "+" || l[0] === "-" ? l[0] : " "}</span>{l.slice(1)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function ThreadView({ thread, owner, name, pr, canPush, onChange, showContext, repoPath }: {
  thread: Thread;
  owner: string;
  name: string;
  pr: FullPull;
  canPush: boolean;
  onChange: () => void;
  /** Show the file name and diff hunk (Conversation tab). */
  showContext?: boolean;
  repoPath: string;
}) {
  const { user } = useAuth();
  const [replying, setReplying] = useState(false);
  const [reply, setReply] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const resolved = !!thread.node?.isResolved;
  const [expanded, setExpanded] = useState(!resolved);
  const root = thread.root;
  const outdated = root.line === null;
  const end = root.line ?? root.original_line ?? 0;
  const start = root.start_line ?? root.original_start_line ?? end;
  const sameRepo = pr.head.repo?.full_name === pr.base.repo?.full_name;
  const canApply = !!user && canPush && sameRepo && pr.state === "open" && !outdated && root.side === "RIGHT";
  const toggleResolve = async () => {
    if (!thread.node) return;
    setErr("");
    try {
      await gql(resolved ? `mutation($t:ID!){unresolveReviewThread(input:{threadId:$t}){thread{id}}}` : `mutation($t:ID!){resolveReviewThread(input:{threadId:$t}){thread{id}}}`, { t: thread.node.id });
      onChange();
    } catch (e) {
      setErr(errorMessage(e));
    }
  };
  return (
    <div className={"box pl-thread" + (resolved ? " pl-thread-resolved" : "")} data-testid="review-thread" data-path={root.path} data-line={end}>
      {showContext && (
        <div className="pl-thread-head">
          <button type="button" className="btn-link mono small pl-thread-path" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
            {root.path}{start !== end ? ` lines ${start}–${end}` : end ? ` line ${end}` : ""}
          </button>
          {outdated && <span className="badge badge-warn">Outdated</span>}
          {resolved && <span className="badge">Resolved</span>}
        </div>
      )}
      {!showContext && resolved && (
        <div className="pl-thread-head">
          <button type="button" className="btn-link small" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
            {expanded ? "Hide resolved" : "Show resolved"}
          </button>
          <span className="muted small">{thread.node?.resolvedBy?.login} marked this conversation as resolved.</span>
        </div>
      )}
      {expanded && (
        <>
          {showContext && <HunkPreview c={root} />}
          {thread.comments.map((c) => {
            const mine = user?.login === c.user?.login;
            return (
              <div key={c.id} className="pl-thread-comment" id={`discussion_r${c.id}`} data-testid="review-comment">
                <div className="pl-thread-comment-head">
                  <Avatar user={c.user} size={24} /> <UserLink user={c.user} /> <RelTime iso={c.created_at} />
                  <span className="spacer" />
                  <KebabMenu label="Review comment actions" items={[
                    { label: "Delete", danger: true, hidden: !(mine || canPush), onClick: async () => {
                      if (!confirmAction("Are you sure you want to delete this comment?")) return;
                      try { await papi.deleteReviewComment(owner, name, c.id); onChange(); } catch (e) { setErr(errorMessage(e)); }
                    } },
                  ]} />
                </div>
                <div className="pl-thread-comment-body">
                  <CommentBody body={c.body} repoPath={repoPath} suggestion={c.id === root.id || c.body.includes("```suggestion") ? {
                    original: hunkLines(c.diff_hunk, c.start_line ?? c.original_start_line ?? c.line ?? c.original_line ?? 0, c.line ?? c.original_line ?? 0, c.side),
                    canApply,
                    onApply: async (repl) => {
                      await applySuggestion(pr.head.repo?.owner.login || owner, pr.head.repo?.name || name, pr.head.ref, c.path, c.start_line ?? c.line ?? 0, c.line ?? 0, repl);
                      onChange();
                    },
                  } : undefined} />
                </div>
                <ReactionBar owner={owner} name={name} subject={`pulls/comments/${c.id}`} summary={c.reactions} />
              </div>
            );
          })}
          {err && <p className="c-danger small pl-thread-err" role="alert">{err}</p>}
          <div className="pl-thread-foot">
            {replying ? (
              <form className="grow" aria-label="Reply to thread" onSubmit={async (e) => {
                e.preventDefault();
                setBusy(true);
                setErr("");
                try {
                  await papi.replyReviewComment(owner, name, pr.number, root.id, reply);
                  setReply("");
                  setReplying(false);
                  onChange();
                } catch (ex) {
                  setErr(errorMessage(ex));
                } finally {
                  setBusy(false);
                }
              }}>
                <MarkdownEditor value={reply} onChange={setReply} label="Reply" placeholder="Reply…" rows={3} repoPath={repoPath} />
                <div className="form-actions">
                  <button type="button" className="btn btn-sm" onClick={() => setReplying(false)}>Cancel</button>
                  <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !reply.trim()}>Reply</button>
                </div>
              </form>
            ) : (
              <>
                {user && <button type="button" className="input pl-reply-btn" onClick={() => setReplying(true)} data-testid="thread-reply">Reply…</button>}
                {thread.node && (resolved ? thread.node.viewerCanUnresolve : thread.node.viewerCanResolve) && (
                  <button type="button" className="btn btn-sm" onClick={toggleResolve} data-testid="thread-resolve">
                    {resolved ? "Unresolve conversation" : "Resolve conversation"}
                  </button>
                )}
              </>
            )}
          </div>
        </>
      )}
    </div>
  );
}

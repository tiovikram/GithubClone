// Repository discussions (GraphQL): `/{o}/{r}/discussions` (+ `/categories/{slug}`), `/new`,
// `/{n}` with threaded comments, answers, close/reopen, lock/unlock, edit and delete.
import { useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { errorMessage } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, ErrorBox, Flash, RelTime, UserLink } from "../../components/common";
import { CommentForm, MarkdownEditor } from "../../components/Conversation";
import { CheckIcon, CommentIcon, LockIcon } from "../../components/icons";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { gql } from "./api";
import { confirmAction, KebabMenu, Popover } from "./ui";

interface Category { id: string; name: string; slug: string; emoji: string; description: string | null; isAnswerable: boolean }
interface Author { login: string; avatarUrl: string }
interface DComment {
  id: string; databaseId: number; body: string; createdAt: string; isAnswer: boolean; viewerCanUpdate: boolean;
  author: Author | null; replies?: { nodes: DComment[] };
}
interface DSummary {
  id: string; number: number; title: string; createdAt: string; updatedAt: string; author: Author | null;
  category: Category; isAnswered: boolean; closed: boolean; locked: boolean; comments: { totalCount: number };
}
interface DFull extends DSummary {
  body: string; viewerCanUpdate: boolean; stateReason: string | null; answer: { id: string } | null;
  comments: { totalCount: number; nodes: DComment[] };
}

const author = (a: Author | null) => (a ? { login: a.login, avatar_url: a.avatarUrl } : null);
const CAT = "id name slug emoji description isAnswerable";
const emoji = (e: string) => {
  const map: Record<string, string> = { ":speech_balloon:": "💬", ":mega:": "📣", ":bulb:": "💡", ":pray:": "🙏", ":raised_hands:": "🙌", ":ballot_box:": "🗳️", ":hash:": "#️⃣" };
  return map[e] || (e.startsWith(":") ? "💬" : e);
};

function useCategories(owner: string, name: string) {
  return useAsync(async () => {
    const d = await gql<{ repository: { id: string; hasDiscussionsEnabled: boolean; discussionCategories: { nodes: Category[] } } | null }>(
      `query($o:String!,$r:String!){repository(owner:$o,name:$r,followRenames:true){id hasDiscussionsEnabled discussionCategories(first:50){nodes{${CAT}}}}}`, { o: owner, r: name });
    return d.repository;
  }, [owner, name]);
}

export function DiscussionsPage() {
  const { repo, owner, name, base } = useRepo();
  const { user } = useAuth();
  const slug = useParams().slug;
  const [sp, setSp] = useSearchParams();
  const state = sp.get("state") === "closed" ? "CLOSED" : "OPEN";
  const cats = useCategories(owner, name);
  const category = (cats.data?.discussionCategories.nodes || []).find((c) => c.slug === slug);
  useTitle(`Discussions · ${repo.full_name}`);
  const list = useAsync(async () => {
    if (!cats.data) return null;
    if (slug && !category) return [] as DSummary[];
    const d = await gql<{ repository: { discussions: { nodes: DSummary[] } } }>(
      `query($o:String!,$r:String!,$c:ID,$s:[DiscussionState!]){repository(owner:$o,name:$r,followRenames:true){discussions(first:50,categoryId:$c,states:$s,orderBy:{field:UPDATED_AT,direction:DESC}){nodes{id number title createdAt updatedAt author{login avatarUrl} category{${CAT}} isAnswered closed locked comments{totalCount}}}}}`,
      { o: owner, r: name, c: category?.id || null, s: [state] });
    return d.repository.discussions.nodes;
  }, [cats.data, slug, state]);
  return (
    <Async state={cats}>
      {(r) => !r || !r.hasDiscussionsEnabled ? (
        <Blankslate title="Discussions are not enabled for this repository">A repository admin can turn on Discussions in the repository settings.</Blankslate>
      ) : (
        <div className="layout-sidebar pl-discussions">
          <aside className="layout-aside pl-disc-cats">
            {user && <Link to={`${base}/discussions/new${category ? `?category=${category.slug}` : ""}`} className="btn btn-primary btn-block" data-testid="new-discussion">New discussion</Link>}
            <h3 className="h6 pl-section-title">Categories</h3>
            <nav className="pl-cat-nav" aria-label="Categories">
              <Link to={`${base}/discussions`} className={"menu-item" + (!slug ? " selected" : "")}>💬 View all discussions</Link>
              {r.discussionCategories.nodes.map((c) => (
                <Link key={c.id} to={`${base}/discussions/categories/${c.slug}`} className={"menu-item" + (slug === c.slug ? " selected" : "")} data-testid={`cat-${c.slug}`}>
                  {emoji(c.emoji)} {c.name}
                </Link>
              ))}
            </nav>
          </aside>
          <div className="layout-main">
            {category && <div className="pl-cat-head"><h2 className="h4">{emoji(category.emoji)} {category.name}</h2>{category.description && <p className="muted small">{category.description}</p>}</div>}
            <div className="box">
              <div className="box-header list-tabs" role="tablist" aria-label="Discussion state">
                <button type="button" role="tab" aria-selected={state === "OPEN"} className={state === "OPEN" ? "selected" : ""} onClick={() => setSp({})}>Open</button>
                <button type="button" role="tab" aria-selected={state === "CLOSED"} className={state === "CLOSED" ? "selected" : ""} onClick={() => setSp({ state: "closed" })}>Closed</button>
              </div>
              <Async state={list}>
                {(items) => !items || items.length === 0 ? <Blankslate title="There aren't any discussions here yet." icon={<CommentIcon size={24} />} /> : (
                  <ul className="list">
                    {items.map((d) => (
                      <li key={d.id} className="list-row pl-disc-row" data-testid="discussion-row" data-title={d.title}>
                        <span className="pl-disc-emoji" aria-hidden="true">{emoji(d.category.emoji)}</span>
                        <div className="grow">
                          <Link to={`${base}/discussions/${d.number}`} className="issue-title">{d.title}</Link>{" "}
                          {d.isAnswered && <span className="badge pl-answered"><CheckIcon /> Answered</span>}{" "}
                          {d.locked && <span className="badge"><LockIcon /> Locked</span>}
                          <div className="muted small"><UserLink user={author(d.author)} /> started <RelTime iso={d.createdAt} /> in {d.category.name}</div>
                        </div>
                        <span className="muted small issue-comments"><CommentIcon /> {d.comments.totalCount}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </Async>
            </div>
          </div>
        </div>
      )}
    </Async>
  );
}

export function NewDiscussionPage() {
  const { repo, owner, name, base } = useRepo();
  const [sp] = useSearchParams();
  const cats = useCategories(owner, name);
  const nav = useNavigate();
  const [cat, setCat] = useState(sp.get("category") || "");
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  useTitle(`New discussion · ${repo.full_name}`);
  return (
    <Async state={cats}>
      {(r) => {
        if (!r) return <Blankslate title="Not found" />;
        const chosen = r.discussionCategories.nodes.find((c) => c.slug === cat);
        if (!chosen) {
          return (
            <div className="narrow-form">
              <h2 className="h3">Start a new discussion</h2>
              <p className="muted">Select a category</p>
              <div className="box">
                <ul className="list">
                  {r.discussionCategories.nodes.map((c) => (
                    <li key={c.id} className="list-row">
                      <span aria-hidden="true">{emoji(c.emoji)}</span>
                      <div className="grow"><strong>{c.name}</strong>{c.description && <div className="muted small">{c.description}</div>}</div>
                      <button type="button" className="btn btn-sm" onClick={() => setCat(c.slug)} data-testid={`choose-${c.slug}`}>Get started</button>
                    </li>
                  ))}
                </ul>
              </div>
            </div>
          );
        }
        return (
          <form className="narrow-form" onSubmit={async (e) => {
            e.preventDefault();
            setBusy(true);
            setErr("");
            try {
              const d = await gql<{ createDiscussion: { discussion: { number: number } } }>(
                `mutation($r:ID!,$c:ID!,$t:String!,$b:String!){createDiscussion(input:{repositoryId:$r,categoryId:$c,title:$t,body:$b}){discussion{number}}}`,
                { r: r.id, c: chosen.id, t: title.trim(), b: body });
              nav(`${base}/discussions/${d.createDiscussion.discussion.number}`);
            } catch (ex) {
              setErr(errorMessage(ex));
              setBusy(false);
            }
          }}>
            <h2 className="h3">Start a new discussion</h2>
            <p>Category: <strong>{emoji(chosen.emoji)} {chosen.name}</strong> <button type="button" className="btn-link small" onClick={() => setCat("")}>Change</button></p>
            {err && <Flash kind="error">{err}</Flash>}
            <div className="form-group">
              <label htmlFor="disc-title">Title</label>
              <input id="disc-title" className="input input-lg" required value={title} onChange={(e) => setTitle(e.target.value)} />
            </div>
            <div className="form-group">
              <span className="label-text">Body</span>
              <MarkdownEditor value={body} onChange={setBody} label="Body" rows={10} repoPath={repo.full_name} />
            </div>
            <div className="form-actions">
              <button type="submit" className="btn btn-primary" disabled={busy || !title.trim() || !body.trim()}>{busy ? "Starting…" : "Start discussion"}</button>
            </div>
          </form>
        );
      }}
    </Async>
  );
}

const COMMENT_FIELDS = "id databaseId body createdAt isAnswer viewerCanUpdate author{login avatarUrl}";

function DiscussionComment({ c, d, canTriage, reload, repoPath, isReply }: {
  c: DComment; d: DFull; canTriage: boolean; reload: () => void; repoPath: string; isReply?: boolean;
}) {
  const { user } = useAuth();
  const [editing, setEditing] = useState(false);
  const [body, setBody] = useState(c.body);
  const [replying, setReplying] = useState(false);
  const [err, setErr] = useState("");
  const canAnswer = d.category.isAnswerable && !isReply && (canTriage || user?.login === d.author?.login);
  const run = async (q: string, v: Record<string, unknown>) => {
    setErr("");
    try { await gql(q, v); reload(); } catch (e) { setErr(errorMessage(e)); }
  };
  return (
    <div className={"pl-dcomment box" + (c.isAnswer ? " pl-answer" : "") + (isReply ? " pl-reply" : "")} data-testid={isReply ? "discussion-reply" : "discussion-comment"} id={`discussioncomment-${c.databaseId}`}>
      <div className="comment-header">
        <Avatar user={author(c.author)} size={20} /> <UserLink user={author(c.author)} /> <span className="muted"><RelTime iso={c.createdAt} /></span>
        {c.isAnswer && <span className="badge pl-answered"><CheckIcon /> Answer</span>}
        <span className="spacer" />
        {canAnswer && (
          <button type="button" className="btn btn-sm" data-testid={c.isAnswer ? "unmark-answer" : "mark-answer"}
            onClick={() => run(c.isAnswer ? `mutation($id:ID!){unmarkDiscussionCommentAsAnswer(input:{id:$id}){discussion{id}}}` : `mutation($id:ID!){markDiscussionCommentAsAnswer(input:{id:$id}){discussion{id}}}`, { id: c.id })}>
            {c.isAnswer ? "Unmark as answer" : "Mark as answer"}
          </button>
        )}
        <KebabMenu label="Comment actions" items={[
          { label: "Edit", hidden: !c.viewerCanUpdate, onClick: () => { setBody(c.body); setEditing(true); } },
          { label: "Delete", danger: true, hidden: !c.viewerCanUpdate, onClick: () => {
            if (confirmAction("Delete this comment?")) run(`mutation($id:ID!){deleteDiscussionComment(input:{id:$id}){comment{id}}}`, { id: c.id });
          } },
        ]} />
      </div>
      {editing ? (
        <form className="comment-body" onSubmit={async (e) => {
          e.preventDefault();
          await run(`mutation($id:ID!,$b:String!){updateDiscussionComment(input:{commentId:$id,body:$b}){comment{id}}}`, { id: c.id, b: body });
          setEditing(false);
        }}>
          <MarkdownEditor value={body} onChange={setBody} label="Edit comment" repoPath={repoPath} />
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setEditing(false)}>Cancel</button>
            <button type="submit" className="btn btn-primary">Update comment</button>
          </div>
        </form>
      ) : <div className="comment-body"><Markdown source={c.body} comment repoPath={repoPath} /></div>}
      {err && <p className="c-danger small pl-pad" role="alert">{err}</p>}
      {!isReply && (
        <div className="pl-replies">
          {(c.replies?.nodes || []).map((r) => <DiscussionComment key={r.id} c={r} d={d} canTriage={canTriage} reload={reload} repoPath={repoPath} isReply />)}
          {user && (!d.locked || canTriage) && (replying ? (
            <ReplyForm repoPath={repoPath} onCancel={() => setReplying(false)} onSubmit={async (b) => {
              await gql(`mutation($d:ID!,$b:String!,$r:ID){addDiscussionComment(input:{discussionId:$d,body:$b,replyToId:$r}){comment{id}}}`, { d: d.id, b, r: c.id });
              setReplying(false);
              reload();
            }} />
          ) : <button type="button" className="input pl-reply-btn" onClick={() => setReplying(true)} data-testid="discussion-reply-open">Write a reply</button>)}
        </div>
      )}
    </div>
  );
}

function ReplyForm({ onSubmit, onCancel, repoPath }: { onSubmit: (b: string) => Promise<void>; onCancel: () => void; repoPath: string }) {
  const [b, setB] = useState("");
  const [err, setErr] = useState("");
  return (
    <form aria-label="Reply" onSubmit={async (e) => { e.preventDefault(); try { await onSubmit(b); } catch (ex) { setErr(errorMessage(ex)); } }}>
      <MarkdownEditor value={b} onChange={setB} label="Reply" rows={3} repoPath={repoPath} />
      {err && <p className="c-danger small" role="alert">{err}</p>}
      <div className="form-actions">
        <button type="button" className="btn btn-sm" onClick={onCancel}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={!b.trim()}>Reply</button>
      </div>
    </form>
  );
}

export function DiscussionPage() {
  const { repo, owner, name, base, canPush } = useRepo();
  const n = Number(useParams().number);
  const nav = useNavigate();
  const { user } = useAuth();
  const st = useAsync(async () => {
    const d = await gql<{ repository: { discussion: DFull | null; discussionCategories: { nodes: Category[] } } | null }>(
      `query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r,followRenames:true){discussionCategories(first:50){nodes{${CAT}}} discussion(number:$n){id number title body createdAt updatedAt viewerCanUpdate stateReason author{login avatarUrl} category{${CAT}} isAnswered answer{id} closed locked
        comments(first:100){totalCount nodes{${COMMENT_FIELDS} replies(first:100){nodes{${COMMENT_FIELDS}}}}}}}}`, { o: owner, r: name, n });
    return d.repository;
  }, [owner, name, n]);
  const [err, setErr] = useState("");
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  useTitle(st.data?.discussion ? `${st.data.discussion.title} · Discussion #${n} · ${repo.full_name}` : `Discussion #${n}`);
  const run = async (q: string, v: Record<string, unknown>) => {
    setErr("");
    try { await gql(q, v); st.reload(); } catch (e) { setErr(errorMessage(e)); }
  };
  return (
    <Async state={st}>
      {(r) => {
        const d = r?.discussion;
        if (!d) return <Blankslate title="Discussion not found" />;
        const canTriage = canPush;
        return (
          <div className="pl-discussion">
            <div className="title-row">
              {editing ? (
                <form className="title-edit grow" onSubmit={async (e) => {
                  e.preventDefault();
                  await run(`mutation($id:ID!,$t:String,$b:String){updateDiscussion(input:{discussionId:$id,title:$t,body:$b}){discussion{id}}}`, { id: d.id, t: title, b: body });
                  setEditing(false);
                }}>
                  <label htmlFor="disc-edit-title" className="sr-only">Title</label>
                  <input id="disc-edit-title" className="input input-lg" value={title} onChange={(e) => setTitle(e.target.value)} />
                  <MarkdownEditor value={body} onChange={setBody} label="Body" repoPath={repo.full_name} />
                  <div className="form-actions">
                    <button type="button" className="btn" onClick={() => setEditing(false)}>Cancel</button>
                    <button type="submit" className="btn btn-primary">Save</button>
                  </div>
                </form>
              ) : (
                <h1 className="issue-headline">{d.title} <span className="muted">#{d.number}</span></h1>
              )}
              <div className="pl-title-actions">
                {d.viewerCanUpdate && !editing && <button type="button" className="btn btn-sm" onClick={() => { setTitle(d.title); setBody(d.body); setEditing(true); }}>Edit</button>}
                {user && <Link to={`${base}/discussions/new`} className="btn btn-sm btn-primary">New discussion</Link>}
              </div>
            </div>
            <div className="issue-meta">
              <span className={"state " + (d.closed ? "state-done" : "state-open")}>{d.closed ? "Closed" : d.isAnswered ? "Answered" : "Open"}</span>
              {d.locked && <span className="badge" data-testid="discussion-locked"><LockIcon /> Locked</span>}
              <span className="muted"><UserLink user={author(d.author)} /> started this conversation in <Link to={`${base}/discussions/categories/${d.category.slug}`}>{d.category.name}</Link> · <RelTime iso={d.createdAt} /></span>
            </div>
            {err && <ErrorBox error={err} />}
            <div className="layout-sidebar">
              <div className="layout-main">
                <div className="pl-dcomment box">
                  <div className="comment-header"><Avatar user={author(d.author)} size={20} /> <UserLink user={author(d.author)} /> <span className="muted"><RelTime iso={d.createdAt} /></span></div>
                  <div className="comment-body"><Markdown source={d.body} comment repoPath={repo.full_name} /></div>
                </div>
                <h2 className="h5 pl-section-title">{d.comments.totalCount} comment{d.comments.totalCount === 1 ? "" : "s"}</h2>
                {d.comments.nodes.map((c) => <DiscussionComment key={c.id} c={c} d={d} canTriage={canTriage} reload={st.reload} repoPath={repo.full_name} />)}
                {d.locked && !canTriage ? (
                  <Flash kind="warn"><LockIcon /> This discussion has been locked. Only collaborators can comment.</Flash>
                ) : (
                  <CommentForm repoPath={repo.full_name} label="Add a comment to the discussion" onSubmit={async (b) => {
                    await gql(`mutation($d:ID!,$b:String!){addDiscussionComment(input:{discussionId:$d,body:$b}){comment{id}}}`, { d: d.id, b });
                    st.reload();
                  }} />
                )}
              </div>
              <aside className="layout-aside">
                <section className="sidebar-section">
                  <h3 className="h6">Category</h3>
                  {d.viewerCanUpdate ? (
                    <select className="input input-sm" aria-label="Category" value={d.category.id}
                      onChange={(e) => run(`mutation($id:ID!,$c:ID){updateDiscussion(input:{discussionId:$id,categoryId:$c}){discussion{id}}}`, { id: d.id, c: e.target.value })}>
                      {r!.discussionCategories.nodes.map((c) => <option key={c.id} value={c.id}>{emoji(c.emoji)} {c.name}</option>)}
                    </select>
                  ) : <p>{emoji(d.category.emoji)} {d.category.name}</p>}
                </section>
                {d.viewerCanUpdate && (
                  <section className="sidebar-section pl-side-actions">
                    {d.closed ? (
                      <button type="button" className="btn-link pl-side-action" data-testid="reopen-discussion"
                        onClick={() => run(`mutation($id:ID!){reopenDiscussion(input:{discussionId:$id}){discussion{id}}}`, { id: d.id })}>Reopen discussion</button>
                    ) : (
                      <Popover label="Close discussion" button={<>Close discussion ▾</>} className="btn-link pl-side-action" testid="close-discussion" align="left">
                        {(close) => (
                          <div className="pl-menu" role="menu">
                            {[["RESOLVED", "Close as resolved"], ["OUTDATED", "Close as outdated"], ["DUPLICATE", "Close as duplicate"]].map(([k, l]) => (
                              <button key={k} type="button" role="menuitem" className="menu-item" onClick={() => { close(); run(`mutation($id:ID!,$r:DiscussionCloseReason){closeDiscussion(input:{discussionId:$id,reason:$r}){discussion{id}}}`, { id: d.id, r: k }); }}>{l}</button>
                            ))}
                          </div>
                        )}
                      </Popover>
                    )}
                    {canTriage && (
                      <button type="button" className="btn-link pl-side-action" data-testid="lock-discussion"
                        onClick={() => run(d.locked ? `mutation($id:ID!){unlockLockable(input:{lockableId:$id}){clientMutationId}}` : `mutation($id:ID!){lockLockable(input:{lockableId:$id}){clientMutationId}}`, { id: d.id })}>
                        <LockIcon /> {d.locked ? "Unlock conversation" : "Lock conversation"}
                      </button>
                    )}
                    <button type="button" className="btn-link pl-side-action c-danger" data-testid="delete-discussion" onClick={async () => {
                      if (!confirmAction("Delete this discussion? This cannot be undone.")) return;
                      try { await gql(`mutation($id:ID!){deleteDiscussion(input:{id:$id}){discussion{id}}}`, { id: d.id }); nav(`${base}/discussions`); } catch (e) { setErr(errorMessage(e)); }
                    }}>Delete discussion</button>
                  </section>
                )}
              </aside>
            </div>
          </div>
        );
      }}
    </Async>
  );
}

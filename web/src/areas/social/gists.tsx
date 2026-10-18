// Gists: /gists (discover, yours, starred, new), /gist/{login}, /gist/{login}/{id}[/edit|/revisions|/forks|/{sha}].
import { useMemo, useState } from "react";
import { Link, Navigate, NavLink, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { errorMessage, type Page } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, CopyButton, ErrorBox, Flash, Pagination, RelTime, Spinner } from "../../components/common";
import { ForkIcon, HistoryIcon, StarIcon, TrashIcon } from "../../components/icons";
import { highlightLines } from "../../highlight";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { sapi, type Gist, type GistComment, type GistFile, type GistHistory } from "./api";
import { GistIcon } from "./icons";
import { MarkdownEditor } from "./markdown-editor";

const fileList = (g: Gist) => Object.values(g.files).filter((f): f is GistFile => !!f);
const title = (g: Gist) => fileList(g)[0]?.filename || g.id;

function GistsNav() {
  const { user } = useAuth();
  const cls = ({ isActive }: { isActive: boolean }) => "tabnav-tab" + (isActive ? " selected" : "");
  return (
    <nav className="tabnav gists-nav" aria-label="Gists">
      <NavLink to="/gists/discover" className={cls} data-testid="gists-nav-discover">All gists</NavLink>
      {user && <NavLink to={`/gist/${user.login}`} end className={cls} data-testid="gists-nav-mine">Your gists</NavLink>}
      {user && <NavLink to="/gists/starred" className={cls} data-testid="gists-nav-starred">Starred</NavLink>}
      {user && <span className="tabnav-extra"><Link to="/gists/new" className="btn btn-sm btn-primary" data-testid="new-gist-button">+ New gist</Link></span>}
    </nav>
  );
}

function GistSnippet({ g }: { g: Gist }) {
  const files = fileList(g);
  return (
    <li className="gist-snippet" data-testid="gist-item">
      <div className="gist-snippet-head">
        <Avatar user={g.owner} size={24} />
        <div>
          <Link to={`/${g.owner.login}`}>{g.owner.login}</Link> / <Link to={`/gist/${g.owner.login}/${g.id}`} className="gist-title"><strong>{title(g)}</strong></Link>
          {!g.public && <span className="badge">Secret</span>}
          <div className="muted small">Created <RelTime iso={g.created_at} />{g.updated_at !== g.created_at && <> · updated <RelTime iso={g.updated_at} /></>}</div>
          {g.description && <div className="small">{g.description}</div>}
        </div>
        <span className="spacer" />
        <span className="muted small gist-snippet-meta">
          {files.length} file{files.length === 1 ? "" : "s"} · {g.comments} comment{g.comments === 1 ? "" : "s"}
        </span>
      </div>
    </li>
  );
}

function GistListView({ state, page, setPage, empty }: {
  state: { data: Page<Gist[]> | undefined; error: unknown; loading: boolean; reload: () => void };
  page: number; setPage: (n: number) => void; empty: string;
}) {
  return (
    <>
      <Async state={state}>
        {(p) => p.items.length === 0 ? <Blankslate icon={<GistIcon size={24} />} title={empty} /> : (
          <ul className="gist-list" data-testid="gist-list">{p.items.map((g) => <GistSnippet key={g.id} g={g} />)}</ul>
        )}
      </Async>
      {state.data && <Pagination page={page} next={state.data.next} prev={state.data.prev} last={state.data.last} onPage={setPage} />}
    </>
  );
}

function usePage(): [number, (n: number) => void] {
  const [sp, setSp] = useSearchParams();
  return [Number(sp.get("page") || 1), (n) => setSp({ page: String(n) })];
}

export function GistsHome() {
  const { user, loading } = useAuth();
  if (loading) return <Spinner />;
  return <Navigate to={user ? `/gist/${user.login}` : "/gists/discover"} replace />;
}

export function DiscoverGists() {
  const [page, setPage] = usePage();
  useTitle("Discover gists");
  const st = useAsync(() => sapi.publicGists(page), [page]);
  return (
    <div className="container gists-page">
      <GistsNav />
      <h1 className="h3">Discover gists</h1>
      <GistListView state={st} page={page} setPage={setPage} empty="There are no public gists yet." />
    </div>
  );
}

export function StarredGists() {
  const { user, loading } = useAuth();
  const [page, setPage] = usePage();
  useTitle("Starred gists");
  const st = useAsync(() => (user ? sapi.starredGists(page) : Promise.resolve({ items: [], links: {} } as Page<Gist[]>)), [user?.login, page]);
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Fgists%2Fstarred" replace />;
  return (
    <div className="container gists-page">
      <GistsNav />
      <h1 className="h3">Starred gists</h1>
      <GistListView state={st} page={page} setPage={setPage} empty="You haven't starred any gists yet." />
    </div>
  );
}

export function UserGists() {
  const { login = "" } = useParams();
  const { user } = useAuth();
  const [page, setPage] = usePage();
  useTitle(`${login}'s gists`);
  const mine = user?.login.toLowerCase() === login.toLowerCase();
  // GET /gists (the caller's own) includes secret gists; /users/{u}/gists only public ones.
  const st = useAsync(() => (mine ? sapi.myGists(page) : sapi.userGists(login, page)), [login, page, mine]);
  return (
    <div className="container gists-page">
      <GistsNav />
      <div className="gists-user-head">
        <h1 className="h3"><Link to={`/${login}`}>{login}</Link>'s gists</h1>
      </div>
      <GistListView state={st} page={page} setPage={setPage} empty={mine ? "You don't have any gists yet." : `${login} doesn't have any public gists yet.`} />
    </div>
  );
}

// ---------- create / edit ----------

interface DraftFile { key: number; orig?: string; filename: string; content: string }
let seq = 0;

function GistForm({ initial, onSubmit, submitLabels, busy }: {
  initial?: Gist;
  onSubmit: (d: { description: string; files: DraftFile[]; deleted: string[]; public: boolean }) => void;
  submitLabels: "create" | "update";
  busy: boolean;
}) {
  const [description, setDescription] = useState(initial?.description || "");
  const [files, setFiles] = useState<DraftFile[]>(() =>
    initial ? fileList(initial).map((f) => ({ key: ++seq, orig: f.filename, filename: f.filename, content: f.content || "" })) : [{ key: ++seq, filename: "", content: "" }],
  );
  const [deleted, setDeleted] = useState<string[]>([]);
  const upd = (k: number, p: Partial<DraftFile>) => setFiles(files.map((f) => (f.key === k ? { ...f, ...p } : f)));
  const valid = files.some((f) => f.content.trim());
  return (
    <form className="gist-form" onSubmit={(e) => { e.preventDefault(); onSubmit({ description, files, deleted, public: false }); }}>
      <label htmlFor="gist-desc" className="sr-only">Gist description</label>
      <input id="gist-desc" className="input" placeholder="Gist description…" value={description} onChange={(e) => setDescription(e.target.value)} />
      {files.map((f, i) => (
        <div key={f.key} className="box gist-file-edit" data-testid="gist-file-editor">
          <div className="box-header gist-file-edit-head">
            <label htmlFor={`gist-fn-${f.key}`} className="sr-only">Filename</label>
            <input id={`gist-fn-${f.key}`} className="input input-sm" placeholder="Filename including extension…" value={f.filename}
              onChange={(e) => upd(f.key, { filename: e.target.value })} data-testid="gist-filename" />
            {files.length > 1 && (
              <button type="button" className="btn btn-sm btn-danger" aria-label={`Remove file ${f.filename || i + 1}`} data-testid="gist-remove-file"
                onClick={() => { setFiles(files.filter((x) => x.key !== f.key)); if (f.orig) setDeleted([...deleted, f.orig]); }}>
                <TrashIcon />
              </button>
            )}
          </div>
          <label htmlFor={`gist-content-${f.key}`} className="sr-only">File contents</label>
          <textarea id={`gist-content-${f.key}`} className="input code-input" rows={12} value={f.content} spellCheck={false}
            onChange={(e) => upd(f.key, { content: e.target.value })} data-testid="gist-content" />
        </div>
      ))}
      <div className="toolbar">
        <button type="button" className="btn" onClick={() => setFiles([...files, { key: ++seq, filename: "", content: "" }])} data-testid="gist-add-file">Add file</button>
        <span className="spacer" />
        {submitLabels === "create" ? (
          <>
            <button type="button" className="btn" disabled={busy || !valid} data-testid="create-secret-gist"
              onClick={() => onSubmit({ description, files, deleted, public: false })}>Create secret gist</button>
            <button type="button" className="btn btn-primary" disabled={busy || !valid} data-testid="create-public-gist"
              onClick={() => onSubmit({ description, files, deleted, public: true })}>Create public gist</button>
          </>
        ) : (
          <button type="submit" className="btn btn-primary" disabled={busy || !valid} data-testid="update-gist">Update {initial?.public ? "public" : "secret"} gist</button>
        )}
      </div>
    </form>
  );
}

const nameOf = (f: DraftFile, i: number) => f.filename.trim() || `gistfile${i + 1}.txt`;

export function NewGistPage() {
  const { user, loading } = useAuth();
  const nav = useNavigate();
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>();
  useTitle("New gist");
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Fgists%2Fnew" replace />;
  return (
    <div className="container gists-page">
      <GistsNav />
      <h1 className="h3">Create a new gist</h1>
      <p className="muted">Instantly share code, notes, and snippets.</p>
      {err !== undefined && <ErrorBox error={err} />}
      <GistForm submitLabels="create" busy={busy} onSubmit={async (d) => {
        setBusy(true);
        setErr(undefined);
        try {
          const files: Record<string, { content: string }> = {};
          d.files.filter((f) => f.content.trim()).forEach((f, i) => { files[nameOf(f, i)] = { content: f.content }; });
          const g = await sapi.createGist({ description: d.description, public: d.public, files });
          nav(`/gist/${g.owner.login}/${g.id}`);
        } catch (e) {
          setErr(e);
        } finally {
          setBusy(false);
        }
      }} />
    </div>
  );
}

export function EditGistPage() {
  const { login = "", id = "" } = useParams();
  const { user } = useAuth();
  const nav = useNavigate();
  const st = useAsync(() => sapi.gist(id), [id]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>();
  useTitle("Editing gist");
  return (
    <div className="container gists-page">
      <Async state={st}>
        {(g) => user?.login !== g.owner.login ? <Navigate to={`/gist/${login}/${id}`} replace /> : (
          <>
            <GistHeader g={g} />
            {err !== undefined && <ErrorBox error={err} />}
            <GistForm initial={g} submitLabels="update" busy={busy} onSubmit={async (d) => {
              setBusy(true);
              setErr(undefined);
              try {
                const files: Record<string, { content?: string; filename?: string } | null> = {};
                for (const name of d.deleted) files[name] = null;
                d.files.forEach((f, i) => {
                  const name = nameOf(f, i);
                  if (f.orig) files[f.orig] = name !== f.orig ? { filename: name, content: f.content } : { content: f.content };
                  else if (f.content.trim()) files[name] = { content: f.content };
                });
                await sapi.updateGist(g.id, { description: d.description, files });
                nav(`/gist/${g.owner.login}/${g.id}`);
              } catch (e) {
                setErr(e);
              } finally {
                setBusy(false);
              }
            }} />
            <Link to={`/gist/${g.owner.login}/${g.id}`} className="btn btn-sm">Cancel</Link>
          </>
        )}
      </Async>
    </div>
  );
}

// ---------- view ----------

function GistHeader({ g, tab, starred, onStar }: { g: Gist; tab?: string; starred?: boolean; onStar?: () => void }) {
  const { user } = useAuth();
  const nav = useNavigate();
  const [busy, setBusy] = useState(false);
  const mine = user?.login === g.owner.login;
  const base = `/gist/${g.owner.login}/${g.id}`;
  const cls = (k: string) => "tabnav-tab" + (tab === k ? " selected" : "");
  return (
    <div className="gist-head">
      <div className="gist-head-row">
        <Avatar user={g.owner} size={32} />
        <div className="gist-head-title">
          <h1 className="h4">
            <Link to={`/gist/${g.owner.login}`}>{g.owner.login}</Link> / <Link to={base}><strong data-testid="gist-title">{title(g)}</strong></Link>
            {!g.public && <span className="badge" data-testid="gist-secret">Secret</span>}
          </h1>
          <div className="muted small">
            Created <RelTime iso={g.created_at} />
            {g.fork_of && <> · forked from <Link to={`/gist/${g.fork_of.owner.login}/${g.fork_of.id}`}>{g.fork_of.owner.login}/{title(g.fork_of)}</Link></>}
          </div>
        </div>
        <span className="spacer" />
        {tab && (
          <div className="gist-actions">
            {mine && <Link to={`${base}/edit`} className="btn btn-sm" data-testid="edit-gist">Edit</Link>}
            {mine && (
              <button type="button" className="btn btn-sm btn-danger" disabled={busy} data-testid="delete-gist" onClick={async () => {
                if (!confirm("Are you positive you want to delete this Gist?")) return;
                setBusy(true);
                try {
                  await sapi.deleteGist(g.id);
                  nav(`/gist/${g.owner.login}`);
                } catch (e) {
                  alert(errorMessage(e));
                  setBusy(false);
                }
              }}><TrashIcon /> Delete</button>
            )}
            {user && (
              <button type="button" className="btn btn-sm" onClick={onStar} aria-pressed={!!starred} data-testid="star-gist">
                <StarIcon className={starred ? "icon c-star" : "icon"} /> {starred ? "Unstar" : "Star"}
              </button>
            )}
            {user && !mine && (
              <button type="button" className="btn btn-sm" disabled={busy} data-testid="fork-gist" onClick={async () => {
                setBusy(true);
                try {
                  const f = await sapi.forkGist(g.id);
                  nav(`/gist/${f.owner.login}/${f.id}`);
                } catch (e) {
                  alert(errorMessage(e));
                  setBusy(false);
                }
              }}><ForkIcon /> Fork</button>
            )}
          </div>
        )}
      </div>
      {g.description && <p className="gist-desc" data-testid="gist-description">{g.description}</p>}
      {tab && (
        <nav className="tabnav" aria-label="Gist">
          <Link to={base} className={cls("code")} data-testid="gist-tab-code">Code</Link>
          <Link to={`${base}/revisions`} className={cls("revisions")} data-testid="gist-tab-revisions"><HistoryIcon /> Revisions {g.history && <span className="counter">{g.history.length}</span>}</Link>
          <Link to={`${base}/forks`} className={cls("forks")} data-testid="gist-tab-forks"><ForkIcon /> Forks {g.forks && <span className="counter">{g.forks.length}</span>}</Link>
        </nav>
      )}
    </div>
  );
}

function FileView({ f, rawUrl }: { f: GistFile; rawUrl: string }) {
  const [mode, setMode] = useState<"preview" | "code">("preview");
  const isMd = /\.(md|markdown)$/i.test(f.filename);
  const text = f.content || "";
  const lines = useMemo(() => highlightLines(text.endsWith("\n") ? text.slice(0, -1) : text, f.filename), [text, f.filename]);
  return (
    <div className="box gist-file" data-testid="gist-file" id={`file-${f.filename.replace(/[^a-z0-9]+/gi, "-").toLowerCase()}`}>
      <div className="box-header gist-file-head">
        <strong className="gist-file-name" data-testid="gist-file-name">{f.filename}</strong>
        <span className="spacer" />
        {isMd && (
          <div className="btn-group">
            <button type="button" className={"btn btn-sm" + (mode === "preview" ? " selected" : "")} onClick={() => setMode("preview")}>Preview</button>
            <button type="button" className={"btn btn-sm" + (mode === "code" ? " selected" : "")} onClick={() => setMode("code")}>Code</button>
          </div>
        )}
        <a href={rawUrl} className="btn btn-sm" data-testid="gist-raw-link">Raw</a>
      </div>
      {isMd && mode === "preview" ? (
        <div className="box-body"><Markdown source={text} /></div>
      ) : (
        <div className="code-view">
          <table className="code-table hljs">
            <tbody>
              {lines.map((html, i) => (
                <tr key={i}>
                  <td className="ln" data-ln={i + 1} />
                  <td className="code" dangerouslySetInnerHTML={{ __html: html || " " }} />
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/** Same-origin raw path (the API's raw_url uses the configured base URL). */
const rawPath = (f: GistFile) => {
  try {
    return new URL(f.raw_url, location.origin).pathname;
  } catch {
    return f.raw_url;
  }
};

function useStar(id: string) {
  const { user } = useAuth();
  const st = useAsync(() => (user ? sapi.gistStarred(id) : Promise.resolve(false)), [id, user?.login]);
  const toggle = async () => {
    try {
      if (st.data) await sapi.unstarGist(id);
      else await sapi.starGist(id);
      st.setData(!st.data);
    } catch (e) {
      alert(errorMessage(e));
    }
  };
  return { starred: !!st.data, toggle };
}

function ShareBox({ g }: { g: Gist }) {
  const url = `${location.origin}/gist/${g.owner.login}/${g.id}`;
  const clone = g.git_pull_url;
  const [kind, setKind] = useState<"share" | "clone" | "embed">("share");
  const first = fileList(g)[0];
  const value = kind === "share" ? url : kind === "clone" ? clone : `<iframe src="${location.origin}${first ? rawPath(first) : ""}" width="100%" height="300" frameborder="0"></iframe>`;
  return (
    <div className="gist-share" data-testid="gist-share">
      <label htmlFor="gist-share-kind" className="sr-only">Share type</label>
      <select id="gist-share-kind" className="input input-sm" value={kind} onChange={(e) => setKind(e.target.value as typeof kind)} data-testid="gist-share-kind">
        <option value="share">Share</option>
        <option value="clone">Clone via HTTPS</option>
        <option value="embed">Embed</option>
      </select>
      <input className="input input-sm" readOnly value={value} aria-label="Share URL" data-testid="gist-share-url" />
      <CopyButton text={value} />
    </div>
  );
}

export function GistPage() {
  const { id = "", sha } = useParams();
  const { user } = useAuth();
  const st = useAsync(() => (sha ? sapi.gistRevision(id, sha) : sapi.gist(id)), [id, sha]);
  const latest = useAsync(() => (sha ? sapi.gist(id) : Promise.resolve(null)), [id, sha]);
  const star = useStar(id);
  useTitle(st.data ? title(st.data) : "Gist");
  return (
    <div className="container gists-page">
      <Async state={st}>
        {(g) => {
          const head = latest.data || g;
          return (
            <>
              <GistHeader g={head} tab="code" starred={star.starred} onStar={star.toggle} />
              {sha && <Flash kind="info">Viewing revision <code>{sha.slice(0, 7)}</code>. <Link to={`/gist/${g.owner.login}/${g.id}`}>View the latest version</Link></Flash>}
              <ShareBox g={g} />
              {fileList(g).map((f) => <FileView key={f.filename} f={f} rawUrl={rawPath(f)} />)}
              {!sha && <GistComments gist={g} canComment={!!user} />}
            </>
          );
        }}
      </Async>
    </div>
  );
}

function GistComments({ gist, canComment }: { gist: Gist; canComment: boolean }) {
  const { user } = useAuth();
  const st = useAsync(() => sapi.gistComments(gist.id), [gist.id]);
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<number | null>(null);
  const [editBody, setEditBody] = useState("");
  const list = st.data || [];
  const save = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await fn();
      st.reload();
      return true;
    } catch (e) {
      alert(errorMessage(e));
      return false;
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="gist-comments" data-testid="gist-comments">
      <h2 className="h5">{list.length} comment{list.length === 1 ? "" : "s"}</h2>
      {st.error ? <ErrorBox error={st.error} onRetry={st.reload} /> : list.map((c: GistComment) => (
        <div key={c.id} className="box gist-comment" data-testid="gist-comment">
          <div className="box-header gist-comment-head">
            <Avatar user={c.user} size={20} /> <Link to={`/${c.user.login}`}><strong>{c.user.login}</strong></Link>
            <span className="muted small">commented <RelTime iso={c.created_at} /></span>
            <span className="spacer" />
            {user?.login === c.user.login && editing !== c.id && (
              <>
                <button type="button" className="btn-link small" onClick={() => { setEditing(c.id); setEditBody(c.body); }} data-testid="gist-comment-edit">Edit</button>
                <button type="button" className="btn-link small danger" onClick={() => confirm("Delete this comment?") && save(() => sapi.deleteGistComment(gist.id, c.id))}
                  data-testid="gist-comment-delete">Delete</button>
              </>
            )}
          </div>
          <div className="box-body">
            {editing === c.id ? (
              <form onSubmit={async (e) => { e.preventDefault(); if (await save(() => sapi.editGistComment(gist.id, c.id, editBody))) setEditing(null); }}>
                <MarkdownEditor id={`gc-edit-${c.id}`} value={editBody} onChange={setEditBody} label="Edit comment" />
                <div className="toolbar">
                  <button type="button" className="btn btn-sm" onClick={() => setEditing(null)}>Cancel</button>
                  <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !editBody.trim()} data-testid="gist-comment-save">Update comment</button>
                </div>
              </form>
            ) : <Markdown source={c.body} comment />}
          </div>
        </div>
      ))}
      {canComment ? (
        <form className="gist-comment-form" onSubmit={async (e) => { e.preventDefault(); if (await save(() => sapi.addGistComment(gist.id, body))) setBody(""); }}>
          <MarkdownEditor id="gist-new-comment" value={body} onChange={setBody} label="Add a comment" placeholder="Leave a comment" />
          <div className="toolbar"><span className="spacer" />
            <button type="submit" className="btn btn-primary" disabled={busy || !body.trim()} data-testid="gist-comment-submit">Comment</button>
          </div>
        </form>
      ) : <p className="muted"><Link to="/login">Sign in</Link> to comment.</p>}
    </section>
  );
}

// ---------- revisions ----------

/** Minimal line diff (LCS) for small files. */
function lineDiff(a: string, b: string): { t: " " | "+" | "-"; s: string }[] {
  const x = a ? a.replace(/\n$/, "").split("\n") : [];
  const y = b ? b.replace(/\n$/, "").split("\n") : [];
  if (x.length * y.length > 4_000_000) return [...x.map((s) => ({ t: "-" as const, s })), ...y.map((s) => ({ t: "+" as const, s }))];
  const n = x.length, m = y.length;
  const L: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) L[i][j] = x[i] === y[j] ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1]);
  const out: { t: " " | "+" | "-"; s: string }[] = [];
  let i = 0, j = 0;
  while (i < n && j < m) {
    if (x[i] === y[j]) { out.push({ t: " ", s: x[i] }); i++; j++; }
    else if (L[i + 1][j] >= L[i][j + 1]) out.push({ t: "-", s: x[i++] });
    else out.push({ t: "+", s: y[j++] });
  }
  while (i < n) out.push({ t: "-", s: x[i++] });
  while (j < m) out.push({ t: "+", s: y[j++] });
  return out;
}

function RevisionDiff({ id, rev, prev }: { id: string; rev: GistHistory; prev?: GistHistory }) {
  const st = useAsync(async () => {
    const [a, b] = await Promise.all([prev ? sapi.gistRevision(id, prev.version) : Promise.resolve(null), sapi.gistRevision(id, rev.version)]);
    const names = new Set([...Object.keys(a?.files || {}), ...Object.keys(b.files || {})]);
    return [...names].sort().map((n) => ({ name: n, before: a?.files[n]?.content || "", after: b.files[n]?.content || "" })).filter((f) => f.before !== f.after);
  }, [id, rev.version, prev?.version]);
  if (st.error) return <ErrorBox error={st.error} />;
  if (!st.data) return <Spinner />;
  return (
    <>
      {st.data.map((f) => (
        <div key={f.name} className="box gist-rev-file">
          <div className="box-header"><strong>{f.name}</strong> {!f.before ? <span className="badge badge-added">added</span> : !f.after ? <span className="badge badge-removed">removed</span> : null}</div>
          <table className="diff-table gist-diff">
            <tbody>
              {lineDiff(f.before, f.after).map((l, i) => (
                <tr key={i} className={l.t === "+" ? "add" : l.t === "-" ? "del" : ""}>
                  <td className={"gist-diff-mark" + (l.t === "+" ? " diff-add" : l.t === "-" ? " diff-del" : "")}>{l.t}</td>
                  <td className={"code" + (l.t === "+" ? " diff-add" : l.t === "-" ? " diff-del" : "")}>{l.s || " "}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ))}
    </>
  );
}

export function GistRevisionsPage() {
  const { id = "" } = useParams();
  const st = useAsync(() => sapi.gist(id), [id]);
  const star = useStar(id);
  useTitle("Revisions");
  return (
    <div className="container gists-page">
      <Async state={st}>
        {(g) => {
          const hist = g.history || [];
          return (
            <>
              <GistHeader g={g} tab="revisions" starred={star.starred} onStar={star.toggle} />
              <ol className="gist-revisions" data-testid="gist-revisions">
                {hist.map((h, i) => (
                  <li key={h.version} className="gist-revision" data-testid="gist-revision">
                    <div className="gist-revision-head">
                      <Avatar user={h.user} size={20} /> <strong>{h.user?.login || "ghost"}</strong>{" "}
                      {i === hist.length - 1 ? "created" : "revised"} this gist <RelTime iso={h.committed_at} />.{" "}
                      <span className="muted small">
                        <span className="c-add">+{h.change_status.additions}</span> <span className="c-del">−{h.change_status.deletions}</span>
                      </span>
                      <span className="spacer" />
                      <Link to={`/gist/${g.owner.login}/${g.id}/${h.version}`} className="btn btn-sm" data-testid="gist-revision-view">View file @ {h.version.slice(0, 7)}</Link>
                    </div>
                    <RevisionDiff id={g.id} rev={h} prev={hist[i + 1]} />
                  </li>
                ))}
              </ol>
            </>
          );
        }}
      </Async>
    </div>
  );
}

export function GistForksPage() {
  const { id = "" } = useParams();
  const st = useAsync(() => sapi.gist(id), [id]);
  const forks = useAsync(() => sapi.gistForks(id), [id]);
  const star = useStar(id);
  useTitle("Forks");
  return (
    <div className="container gists-page">
      <Async state={st}>
        {(g) => (
          <>
            <GistHeader g={g} tab="forks" starred={star.starred} onStar={star.toggle} />
            <Async state={forks}>
              {(list) => list.length === 0 ? <Blankslate title="No forks yet" /> : (
                <ul className="box list" data-testid="gist-forks">
                  {list.map((f) => (
                    <li key={f.id} className="list-row">
                      <Avatar user={f.owner} size={20} />
                      <Link to={`/gist/${f.owner.login}/${f.id}`}>{f.owner.login} / {title(f)}</Link>
                      <span className="muted small">forked <RelTime iso={f.created_at} /></span>
                    </li>
                  ))}
                </ul>
              )}
            </Async>
          </>
        )}
      </Async>
    </div>
  );
}

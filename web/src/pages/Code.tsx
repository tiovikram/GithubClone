import { useMemo, useState } from "react";
import { Link, Navigate, useNavigate, useParams } from "react-router-dom";
import { api, decodeBase64, looksBinary, type Branch, type ContentItem, type Repo } from "../api";
import { BranchSelect } from "../components/BranchSelect";
import { Async, Avatar, Blankslate, CodeLine, ErrorBox, NotFoundView, RelTime, Spinner } from "../components/common";
import { BookIcon, DirIcon, FileIcon, HistoryIcon } from "../components/icons";
import { useRepo } from "../components/RepoLayout";
import { highlightLines } from "../highlight";
import { useAsync, useTitle } from "../hooks";
import { Markdown } from "../markdown";
import { encPath, firstLine, humanSize, shortSha, splitRefPath } from "../util";

function useRefPath(branches: Branch[] | undefined, repo: Repo) {
  const params = useParams();
  const splat = params["*"] || "";
  return useMemo(() => {
    if (!splat) return { ref: repo.default_branch, path: "" };
    return splitRefPath(splat, (branches || []).map((b) => b.name));
  }, [splat, branches, repo.default_branch]);
}

function Breadcrumbs({ base, refName, path, isFile }: { base: string; refName: string; path: string; isFile?: boolean }) {
  const parts = path.split("/").filter(Boolean);
  const r = encPath(refName);
  return (
    <nav className="breadcrumbs" aria-label="Breadcrumb">
      <Link to={`${base}/tree/${r}`}><strong>{base.split("/")[2]}</strong></Link>
      {parts.map((p, i) => {
        const sub = parts.slice(0, i + 1).join("/");
        const last = i === parts.length - 1;
        return (
          <span key={sub}>
            <span className="sep">/</span>
            {last ? (
              <strong aria-current="page">{p}</strong>
            ) : (
              <Link to={`${base}/tree/${r}/${encPath(sub)}`}>{p}</Link>
            )}
          </span>
        );
      })}
      {!isFile && parts.length > 0 && <span className="sep">/</span>}
    </nav>
  );
}

function LatestCommit({ path, refName }: { path: string; refName: string }) {
  const { owner, name, base } = useRepo();
  const st = useAsync(() => api.commits(owner, name, refName, 1, path || undefined), [owner, name, refName, path]);
  const c = st.data?.items[0];
  return (
    <div className="latest-commit">
      {st.loading ? (
        <span className="muted">Loading latest commit…</span>
      ) : c ? (
        <>
          <span className="latest-commit-msg">
            <Avatar user={c.author} size={20} />{" "}
            <strong>{c.author?.login || c.commit.author?.name}</strong>{" "}
            <Link to={`${base}/commit/${c.sha}`} className="muted-link">{firstLine(c.commit.message)}</Link>
          </span>
          <span className="muted small">
            <Link to={`${base}/commit/${c.sha}`} className="mono muted-link">{shortSha(c.sha)}</Link> ·{" "}
            <RelTime iso={c.commit.committer?.date} />
          </span>
          <Link to={`${base}/commits/${encPath(refName)}${path ? "?path=" + encodeURIComponent(path) : ""}`} className="btn btn-sm">
            <HistoryIcon /> History
          </Link>
        </>
      ) : (
        <span className="muted">No commit information.</span>
      )}
    </div>
  );
}

function CloneBox({ repo }: { repo: Repo }) {
  const url = repo.clone_url || `${location.origin}/${repo.full_name}.git`;
  return (
    <div className="clone-box box">
      <div className="clone-box-title">Clone with HTTPS</div>
      <CodeLine text={url} label="Clone URL" />
      <div className="muted small">
        Use Git or <code>gh repo clone {repo.full_name}</code> with a{" "}
        <Link to="/settings/tokens">personal access token</Link>.
      </div>
    </div>
  );
}

function EmptyRepo({ repo }: { repo: Repo }) {
  const url = repo.clone_url || `${location.origin}/${repo.full_name}.git`;
  const branch = repo.default_branch || "main";
  return (
    <div className="box quick-setup">
      <div className="box-header"><h2 className="h4">Quick setup — this repository is empty</h2></div>
      <div className="box-body">
        <CodeLine text={url} label="Clone URL" />
        <h3 className="h5">…or create a new repository on the command line</h3>
        <pre className="cmd">{`echo "# ${repo.name}" >> README.md
git init
git add README.md
git commit -m "first commit"
git branch -M ${branch}
git remote add origin ${url}
git push -u origin ${branch}`}</pre>
        <h3 className="h5">…or push an existing repository from the command line</h3>
        <pre className="cmd">{`git remote add origin ${url}
git branch -M ${branch}
git push -u origin ${branch}`}</pre>
        <p className="muted small">
          Git will ask for credentials: use your username and a <Link to="/settings/tokens">personal access token</Link> as the password.
        </p>
      </div>
    </div>
  );
}

function sortEntries(items: ContentItem[]) {
  return [...items].sort((a, b) => {
    const da = a.type === "dir" ? 0 : 1;
    const db = b.type === "dir" ? 0 : 1;
    return da - db || a.name.localeCompare(b.name);
  });
}

function Readme({ refName, dir, entries }: { refName: string; dir: string; entries: ContentItem[] }) {
  const { owner, name } = useRepo();
  const readme = entries.find((e) => e.type === "file" && /^readme(\.(md|markdown|txt|rst))?$/i.test(e.name));
  const st = useAsync(
    () => (readme ? (dir ? api.readme(owner, name, refName, dir) : api.readme(owner, name, refName)) : Promise.resolve(null)),
    [owner, name, refName, dir, readme?.path],
  );
  if (!readme) return null;
  const isMd = /\.(md|markdown)$/i.test(readme.name);
  return (
    <div className="box readme" id="readme">
      <div className="box-header"><BookIcon /> <strong>{readme.name}</strong></div>
      <div className="box-body">
        {st.loading ? <Spinner /> : st.error ? (
          <ErrorBox error={st.error} onRetry={st.reload} />
        ) : isMd ? (
          <Markdown source={st.data || ""} />
        ) : (
          <pre className="plain">{st.data}</pre>
        )}
      </div>
    </div>
  );
}

/** `/{o}/{r}` and `/{o}/{r}/tree/{ref}/{path}` */
export function CodePage() {
  const { repo, owner, name } = useRepo();
  const branchesSt = useAsync(() => api.branches(owner, name), [owner, name]);
  useTitle(repo.full_name);
  if (branchesSt.loading && !branchesSt.data) return <Spinner />;
  if (branchesSt.error) return <ErrorBox error={branchesSt.error} onRetry={branchesSt.reload} />;
  const branches = branchesSt.data || [];
  if (branches.length === 0) {
    return (
      <div className="layout-sidebar">
        <div className="layout-main"><EmptyRepo repo={repo} /></div>
      </div>
    );
  }
  return <Tree branches={branches} />;
}

function Tree({ branches }: { branches: Branch[] }) {
  const { repo, owner, name, base } = useRepo();
  const { ref, path } = useRefPath(branches, repo);
  const nav = useNavigate();
  const st = useAsync(() => api.contents(owner, name, path, ref), [owner, name, path, ref]);
  return (
    <div className="layout-sidebar">
      <div className="layout-main">
        <div className="toolbar">
          <BranchSelect branches={branches.map((b) => b.name)} value={ref}
            onChange={(b) => nav(`${base}/tree/${encPath(b)}${path ? "/" + encPath(path) : ""}`)} />
          {path ? <Breadcrumbs base={base} refName={ref} path={path} /> : (
            <span className="muted small">
              <strong>{branches.length}</strong> branch{branches.length === 1 ? "" : "es"}
            </span>
          )}
          <span className="spacer" />
          <Link className="btn btn-sm" to={`${base}/commits/${encPath(ref)}`}><HistoryIcon /> Commits</Link>
        </div>
        <Async state={st} notFound={<Blankslate title="Path not found">
          <code>{path || "/"}</code> does not exist on <code>{ref}</code>.</Blankslate>}>
          {(data) => {
            if (!Array.isArray(data)) {
              // A file path under /tree/ — redirect to the blob view.
              return <Navigate to={`${base}/blob/${encPath(ref)}/${encPath(path)}`} replace />;
            }
            const entries = sortEntries(data);
            const parent = path.split("/").slice(0, -1).join("/");
            return (
              <>
                <div className="box file-tree">
                  <LatestCommit path={path} refName={ref} />
                  <ul className="list" aria-label="Files">
                    {path && (
                      <li className="list-row tree-row">
                        <Link to={`${base}/tree/${encPath(ref)}${parent ? "/" + encPath(parent) : ""}`} aria-label="Parent directory">..</Link>
                      </li>
                    )}
                    {entries.map((e) => (
                      <li key={e.path} className="list-row tree-row">
                        {e.type === "dir" ? <DirIcon /> : <FileIcon className="icon muted" />}
                        <Link to={`${base}/${e.type === "dir" ? "tree" : "blob"}/${encPath(ref)}/${encPath(e.path)}`}>
                          {e.name}
                        </Link>
                        {e.type === "submodule" && <span className="badge">submodule</span>}
                      </li>
                    ))}
                    {entries.length === 0 && <li className="list-row muted">This directory is empty.</li>}
                  </ul>
                </div>
                <Readme refName={ref} dir={path} entries={entries} />
              </>
            );
          }}
        </Async>
      </div>
      {!path && (
        <aside className="layout-aside">
          <section className="about">
            <h2 className="h4">About</h2>
            <p>{repo.description || <span className="muted"><em>No description, website, or topics provided.</em></span>}</p>
            {repo.homepage && <p><a href={repo.homepage} rel="nofollow noopener">{repo.homepage}</a></p>}
          </section>
          <CloneBox repo={repo} />
        </aside>
      )}
    </div>
  );
}

/** `/{o}/{r}/blob/{ref}/{path}` */
export function BlobPage() {
  const { repo, owner, name } = useRepo();
  const branchesSt = useAsync(() => api.branches(owner, name), [owner, name]);
  if (branchesSt.loading && !branchesSt.data) return <Spinner />;
  if (branchesSt.error) return <ErrorBox error={branchesSt.error} onRetry={branchesSt.reload} />;
  return <BlobView branches={branchesSt.data || []} repo={repo} />;
}

function BlobView({ branches, repo }: { branches: Branch[]; repo: Repo }) {
  const { owner, name, base } = useRepo();
  const { ref, path } = useRefPath(branches, repo);
  const nav = useNavigate();
  const fname = path.split("/").pop() || path;
  useTitle(`${fname} at ${ref} · ${repo.full_name}`);
  const st = useAsync(async () => {
    const item = await api.contents(owner, name, path, ref);
    if (Array.isArray(item)) return { dir: true as const };
    let text: string;
    if (item.content && item.encoding === "base64") text = decodeBase64(item.content);
    else text = await api.rawFile(owner, name, path, ref);
    return { dir: false as const, item, text };
  }, [owner, name, path, ref]);
  const isMd = /\.(md|markdown)$/i.test(fname);
  const [mode, setMode] = useState<"preview" | "code">("preview");

  return (
    <div>
      <div className="toolbar">
        <BranchSelect branches={branches.map((b) => b.name)} value={ref}
          onChange={(b) => nav(`${base}/blob/${encPath(b)}/${encPath(path)}`)} />
        <Breadcrumbs base={base} refName={ref} path={path} isFile />
      </div>
      <Async state={st} notFound={<NotFoundView />}>
        {(d) => {
          if (d.dir) {
            return <Navigate to={`${base}/tree/${encPath(ref)}/${encPath(path)}`} replace />;
          }
          const binary = looksBinary(d.text);
          const lines = d.text.endsWith("\n") ? d.text.slice(0, -1).split("\n") : d.text.split("\n");
          return (
            <>
              <LatestCommit path={path} refName={ref} />
              <div className="box blob">
                <div className="box-header blob-header">
                  {isMd && !binary && (
                    <div className="segmented" role="group" aria-label="View mode">
                      <button type="button" aria-pressed={mode === "preview"} onClick={() => setMode("preview")}>Preview</button>
                      <button type="button" aria-pressed={mode === "code"} onClick={() => setMode("code")}>Code</button>
                    </div>
                  )}
                  <span className="muted small">
                    {binary ? "Binary file" : `${lines.length} lines`} · {humanSize(d.item.size)}
                  </span>
                  <span className="spacer" />
                  <button type="button" className="btn btn-sm" onClick={() => {
                    const url = URL.createObjectURL(new Blob([d.text], { type: "text/plain" }));
                    const a = document.createElement("a");
                    a.href = url;
                    a.download = fname;
                    a.click();
                    setTimeout(() => URL.revokeObjectURL(url), 1000);
                  }}>Download</button>
                </div>
                {binary ? (
                  <div className="box-body muted">Binary file not shown.</div>
                ) : isMd && mode === "preview" ? (
                  <div className="box-body"><Markdown source={d.text} /></div>
                ) : (
                  <CodeView text={d.text} filename={fname} />
                )}
              </div>
            </>
          );
        }}
      </Async>
    </div>
  );
}

function CodeView({ text, filename }: { text: string; filename: string }) {
  const lines = useMemo(() => {
    const t = text.endsWith("\n") ? text.slice(0, -1) : text;
    return highlightLines(t, filename);
  }, [text, filename]);
  const hash = typeof location !== "undefined" ? location.hash : "";
  return (
    <div className="code-view">
      <table className="code-table hljs">
        <tbody>
          {lines.map((html, i) => (
            <tr key={i} id={`L${i + 1}`} className={hash === `#L${i + 1}` ? "hl" : undefined}>
              <td className="ln"><a href={`#L${i + 1}`} data-ln={i + 1} aria-label={`Line ${i + 1}`} /></td>
              <td className="code" dangerouslySetInnerHTML={{ __html: html || " " }} />
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}


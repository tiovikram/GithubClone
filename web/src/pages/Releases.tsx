import { useState, type FormEvent } from "react";
import { Link, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api, errorMessage, type Release } from "../api";
import { Async, Blankslate, ErrorBox, Pagination, RelTime, Spinner, UserLink } from "../components/common";
import { MarkdownEditor } from "../components/Conversation";
import { DownloadIcon, TagIcon, TrashIcon } from "../components/icons";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";
import { Markdown } from "../markdown";
import { encPath, humanSize, shortSha } from "../util";

/** Download links for the server-side routes; these are plain <a href>, never router links. */
function sourceArchives(full: string, tag: string) {
  return [
    { name: "Source code (zip)", href: `/${full}/archive/refs/tags/${encPath(tag)}.zip` },
    { name: "Source code (tar.gz)", href: `/${full}/archive/refs/tags/${encPath(tag)}.tar.gz` },
  ];
}

function ReleaseCard({ rel, latest, onChanged }: { rel: Release; latest?: boolean; onChanged?: () => void }) {
  const { repo, owner, name, base, canPush } = useRepo();
  const [files, setFiles] = useState<File[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const upload = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr("");
    try {
      for (const f of files) await api.uploadAsset(rel, f);
      setFiles([]);
      onChanged?.();
    } catch (ex) {
      setErr(errorMessage(ex));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="box release" aria-labelledby={`rel-${rel.id}`} data-testid="release">
      <div className="box-header release-header">
        <h2 className="h3" id={`rel-${rel.id}`}>
          <Link to={`${base}/releases/tag/${encPath(rel.tag_name)}`}>{rel.name || rel.tag_name}</Link>
        </h2>
        {rel.draft && <span className="badge badge-warn">Draft</span>}
        {rel.prerelease && <span className="badge badge-warn">Pre-release</span>}
        {latest && !rel.draft && !rel.prerelease && <span className="badge badge-added">Latest</span>}
      </div>
      <div className="box-body">
        <p className="muted small release-meta">
          <TagIcon /> <Link to={`${base}/tree/${encPath(rel.tag_name)}`} className="mono">{rel.tag_name}</Link>
          {" · "}
          <UserLink user={rel.author} /> released this <RelTime iso={rel.published_at || rel.created_at} />
        </p>
        <Markdown source={rel.body} />
        <h3 className="h5 assets-title">Assets <span className="counter">{rel.assets.length + (rel.draft ? 0 : 2)}</span></h3>
        <ul className="box list assets">
          {rel.assets.map((a) => (
            <li key={a.id} className="list-row asset-row">
              <DownloadIcon />
              <a href={a.browser_download_url.replace(/^https?:\/\/[^/]+/, "")} className="grow" data-testid="asset-link">{a.name}</a>
              <span className="muted small">{humanSize(a.size)}</span>
              {canPush && (
                <button type="button" className="btn btn-sm btn-icon btn-danger-outline" aria-label={`Delete ${a.name}`}
                  onClick={async () => {
                    if (!confirm(`Delete ${a.name}?`)) return;
                    try {
                      await api.deleteAsset(owner, name, a.id);
                      onChanged?.();
                    } catch (ex) {
                      setErr(errorMessage(ex));
                    }
                  }}><TrashIcon /></button>
              )}
            </li>
          ))}
          {!rel.draft && sourceArchives(repo.full_name, rel.tag_name).map((s) => (
            <li key={s.href} className="list-row asset-row">
              <DownloadIcon /> <a href={s.href} className="grow">{s.name}</a>
            </li>
          ))}
        </ul>
        {canPush && onChanged && (
          <form className="asset-upload" onSubmit={upload}>
            <label htmlFor={`upload-${rel.id}`} className="small">Attach binaries</label>
            <input id={`upload-${rel.id}`} type="file" multiple data-testid="asset-input"
              onChange={(e) => setFiles([...(e.target.files || [])])} />
            <button type="submit" className="btn btn-sm" disabled={busy || files.length === 0}>
              {busy ? "Uploading…" : "Upload"}
            </button>
          </form>
        )}
        {err && <p className="c-danger small" role="alert">{err}</p>}
      </div>
    </section>
  );
}

/** `/{o}/{r}/releases` */
export function ReleasesPage() {
  const { repo, owner, name, base, canPush } = useRepo();
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  useTitle(`Releases · ${repo.full_name}`);
  const st = useAsync(() => api.releases(owner, name, page), [owner, name, page]);
  const firstPublished = st.data?.items.find((r) => !r.draft && !r.prerelease)?.id;
  return (
    <div>
      <div className="toolbar">
        <h2 className="h3">Releases</h2>
        <Link to={`${base}/tags`} className="btn btn-sm"><TagIcon /> Tags</Link>
        <span className="spacer" />
        {canPush && <Link to={`${base}/releases/new`} className="btn btn-primary">Draft a new release</Link>}
      </div>
      <Async state={st}>
        {(p) =>
          p.items.length === 0 ? (
            <Blankslate title="There aren't any releases here" icon={<TagIcon size={24} />}>
              Releases are packaged versions of your project with release notes and downloadable files.
            </Blankslate>
          ) : (
            <div className="releases">
              {p.items.map((r) => <ReleaseCard key={r.id} rel={r} latest={page === 1 && r.id === firstPublished} />)}
            </div>
          )
        }
      </Async>
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last}
        onPage={(n) => setSp({ page: String(n) })} />}
    </div>
  );
}

/** `/{o}/{r}/releases/tag/{tag}` and `/{o}/{r}/releases/latest` */
export function ReleasePage({ latest }: { latest?: boolean }) {
  const { repo, owner, name, base, canPush } = useRepo();
  const tag = useParams()["*"] || "";
  const nav = useNavigate();
  const st = useAsync(() => (latest ? api.latestRelease(owner, name) : api.releaseByTag(owner, name, tag)), [owner, name, tag, latest]);
  useTitle(`Release ${st.data?.name || tag} · ${repo.full_name}`);
  const [err, setErr] = useState("");
  if (latest && st.data) return <Navigate to={`${base}/releases/tag/${encPath(st.data.tag_name)}`} replace />;
  return (
    <div>
      <nav className="breadcrumbs small">
        <Link to={`${base}/releases`}>Releases</Link> <span className="sep">/</span> <strong>{st.data?.tag_name || tag}</strong>
      </nav>
      <Async state={st} notFound={<Blankslate title="Release not found">No release exists for <code>{tag}</code>.</Blankslate>}>
        {(rel) => (
          <>
            <ReleaseCard rel={rel} onChanged={st.reload} />
            {canPush && (
              <div className="form-actions">
                {err && <span className="c-danger small" role="alert">{err}</span>}
                {rel.draft && (
                  <button type="button" className="btn btn-primary" onClick={async () => {
                    try {
                      st.setData(await api.updateRelease(owner, name, rel.id, { draft: false }));
                    } catch (e) {
                      setErr(errorMessage(e));
                    }
                  }}>Publish release</button>
                )}
                <button type="button" className="btn btn-danger" onClick={async () => {
                  if (!confirm(`Delete release ${rel.name || rel.tag_name}? The tag is kept.`)) return;
                  try {
                    await api.deleteRelease(owner, name, rel.id);
                    nav(`${base}/releases`);
                  } catch (e) {
                    setErr(errorMessage(e));
                  }
                }}>Delete release</button>
              </div>
            )}
          </>
        )}
      </Async>
    </div>
  );
}

/** `/{o}/{r}/releases/new` */
export function NewReleasePage() {
  const { repo, owner, name, base, canPush } = useRepo();
  const nav = useNavigate();
  useTitle(`New release · ${repo.full_name}`);
  const branches = useAsync(() => api.branches(owner, name), [owner, name]);
  const [tag, setTag] = useState("");
  const [target, setTarget] = useState(repo.default_branch);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [pre, setPre] = useState(false);
  const [files, setFiles] = useState<File[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  if (!canPush) return <Navigate to={`${base}/releases`} replace />;
  const submit = async (draft: boolean) => {
    setBusy(true);
    setError(undefined);
    try {
      const rel = await api.createRelease(owner, name, {
        tag_name: tag.trim(), target_commitish: target, name: title.trim() || undefined, body, draft, prerelease: pre,
      });
      for (const f of files) await api.uploadAsset(rel, f);
      nav(`${base}/releases/tag/${encPath(rel.tag_name)}`);
    } catch (e) {
      setError(e);
      setBusy(false);
    }
  };
  return (
    <form className="narrow-form" onSubmit={(e) => { e.preventDefault(); void submit(false); }}>
      <h2 className="h3">Draft a new release</h2>
      {error !== undefined && <ErrorBox error={error} />}
      <div className="form-row">
        <div className="form-group grow">
          <label htmlFor="rel-tag">Tag</label>
          <input id="rel-tag" className="input" required value={tag} placeholder="v1.0.0" onChange={(e) => setTag(e.target.value)} />
          <p className="small muted">An existing tag, or a new tag created on publish.</p>
        </div>
        <div className="form-group">
          <label htmlFor="rel-target">Target</label>
          {branches.loading ? <Spinner /> : (
            <select id="rel-target" className="input input-auto" value={target} onChange={(e) => setTarget(e.target.value)}>
              {(branches.data || []).map((b) => <option key={b.name} value={b.name}>{b.name} ({shortSha(b.commit.sha)})</option>)}
            </select>
          )}
        </div>
      </div>
      <div className="form-group">
        <label htmlFor="rel-title">Release title</label>
        <input id="rel-title" className="input" value={title} onChange={(e) => setTitle(e.target.value)} />
      </div>
      <div className="form-group">
        <span className="label-text">Describe this release</span>
        <MarkdownEditor value={body} onChange={setBody} label="Release notes" placeholder="Release notes" rows={10} repoPath={repo.full_name} />
      </div>
      <div className="form-group">
        <label htmlFor="rel-files">Attach binaries</label>
        <input id="rel-files" type="file" multiple onChange={(e) => setFiles([...(e.target.files || [])])} />
        {files.length > 0 && <p className="small muted">{files.map((f) => `${f.name} (${humanSize(f.size)})`).join(", ")}</p>}
      </div>
      <label className="check-row">
        <input type="checkbox" checked={pre} onChange={(e) => setPre(e.target.checked)} /> Set as a pre-release
      </label>
      <div className="form-actions">
        <button type="button" className="btn" disabled={busy || !tag.trim()} onClick={() => submit(true)}>Save draft</button>
        <button type="submit" className="btn btn-primary" disabled={busy || !tag.trim()}>
          {busy ? "Publishing…" : "Publish release"}
        </button>
      </div>
    </form>
  );
}

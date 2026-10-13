import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api, errorMessage } from "../api";
import { Async, Blankslate, ErrorBox, Pagination } from "../components/common";
import { BranchIcon, DownloadIcon, TagIcon, TrashIcon } from "../components/icons";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";
import { encPath, shortSha } from "../util";

/** `/{o}/{r}/branches` */
export function BranchesPage() {
  const { repo, owner, name, base, canPush } = useRepo();
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  useTitle(`Branches · ${repo.full_name}`);
  const st = useAsync(() => api.branchesPage(owner, name, page), [owner, name, page]);
  const [err, setErr] = useState<unknown>();
  return (
    <div>
      <div className="toolbar">
        <h2 className="h3">Branches</h2>
        <span className="spacer" />
        <Link to={`${base}/tags`} className="btn btn-sm"><TagIcon /> Tags</Link>
      </div>
      {err !== undefined && <ErrorBox error={err} />}
      <Async state={st}>
        {(p) =>
          p.items.length === 0 ? (
            <Blankslate title="No branches" icon={<BranchIcon size={24} />}>Push a branch to get started.</Blankslate>
          ) : (
            <ul className="box list" aria-label="Branches">
              {p.items.map((b) => {
                const isDefault = b.name === repo.default_branch;
                return (
                  <li key={b.name} className="list-row branch-row" data-testid="branch-row">
                    <BranchIcon />
                    <div className="grow">
                      <Link to={`${base}/tree/${encPath(b.name)}`} className="branch-name">{b.name}</Link>{" "}
                      {isDefault && <span className="badge">default</span>}
                      {b.protected && <span className="badge">protected</span>}
                      <div className="muted small">
                        Last commit <Link className="mono muted-link" to={`${base}/commit/${b.commit.sha}`}>{shortSha(b.commit.sha)}</Link>
                      </div>
                    </div>
                    {!isDefault && (
                      <Link className="btn btn-sm" to={`${base}/compare/${encPath(repo.default_branch)}...${encPath(b.name)}`}>
                        Compare
                      </Link>
                    )}
                    {canPush && !isDefault && (
                      <button type="button" className="btn btn-sm btn-icon btn-danger-outline" aria-label={`Delete branch ${b.name}`}
                        onClick={async () => {
                          if (!confirm(`Delete branch ${b.name}?`)) return;
                          setErr(undefined);
                          try {
                            await api.deleteBranch(owner, name, b.name);
                            st.reload();
                          } catch (e) {
                            setErr(new Error(errorMessage(e)));
                          }
                        }}><TrashIcon /></button>
                    )}
                  </li>
                );
              })}
            </ul>
          )
        }
      </Async>
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last}
        onPage={(n) => setSp({ page: String(n) })} />}
    </div>
  );
}

/** `/{o}/{r}/tags` */
export function TagsPage() {
  const { repo, owner, name, base } = useRepo();
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  useTitle(`Tags · ${repo.full_name}`);
  const st = useAsync(() => api.tags(owner, name, page), [owner, name, page]);
  return (
    <div>
      <div className="toolbar">
        <h2 className="h3">Tags</h2>
        <span className="spacer" />
        <Link to={`${base}/releases`} className="btn btn-sm">Releases</Link>
        <Link to={`${base}/branches`} className="btn btn-sm"><BranchIcon /> Branches</Link>
      </div>
      <Async state={st}>
        {(p) =>
          p.items.length === 0 ? (
            <Blankslate title="There aren't any tags yet" icon={<TagIcon size={24} />} />
          ) : (
            <ul className="box list" aria-label="Tags">
              {p.items.map((t) => (
                <li key={t.name} className="list-row branch-row" data-testid="tag-row">
                  <TagIcon />
                  <div className="grow">
                    <Link to={`${base}/tree/${encPath(t.name)}`} className="branch-name">{t.name}</Link>
                    <div className="muted small">
                      <Link className="mono muted-link" to={`${base}/commit/${t.commit.sha}`}>{shortSha(t.commit.sha)}</Link>
                      {" · "}
                      <a href={`/${repo.full_name}/archive/refs/tags/${encPath(t.name)}.zip`}><DownloadIcon size={12} /> zip</a>
                      {" · "}
                      <a href={`/${repo.full_name}/archive/refs/tags/${encPath(t.name)}.tar.gz`}><DownloadIcon size={12} /> tar.gz</a>
                    </div>
                  </div>
                  <Link className="btn btn-sm" to={`${base}/releases/tag/${encPath(t.name)}`}>Release</Link>
                </li>
              ))}
            </ul>
          )
        }
      </Async>
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last}
        onPage={(n) => setSp({ page: String(n) })} />}
    </div>
  );
}

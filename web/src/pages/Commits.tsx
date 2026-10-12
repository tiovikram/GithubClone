import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api } from "../api";
import { BranchSelect } from "../components/BranchSelect";
import { CommitList } from "../components/CommitList";
import { Async, Avatar, Blankslate, CopyButton, Pagination, RelTime } from "../components/common";
import { DiffView } from "../components/Diff";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";
import { encPath, firstLine, restLines, shortSha } from "../util";

/** `/{o}/{r}/commits/{ref}` (also `/commits` → default branch) */
export function CommitsPage() {
  const { repo, owner, name, base } = useRepo();
  const ref = useParams()["*"] || repo.default_branch;
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  const path = sp.get("path") || undefined;
  const nav = useNavigate();
  useTitle(`Commits · ${repo.full_name}`);
  const branches = useAsync(() => api.branches(owner, name), [owner, name]);
  const st = useAsync(() => api.commits(owner, name, ref, page, path), [owner, name, ref, page, path]);
  return (
    <div>
      <div className="toolbar">
        <h2 className="h3">Commits</h2>
        <span className="spacer" />
        <BranchSelect branches={(branches.data || []).map((b) => b.name)} value={ref}
          onChange={(b) => nav(`${base}/commits/${encPath(b)}`)} />
      </div>
      {path && (
        <p className="muted">
          History for <code>{path}</code> · <Link to={`${base}/commits/${encPath(ref)}`}>show all</Link>
        </p>
      )}
      <Async state={st} notFound={<Blankslate title="No commits">This branch has no commits, or does not exist.</Blankslate>}>
        {(p) =>
          p.items.length === 0 ? (
            <Blankslate title="No commits yet" />
          ) : (
            <>
              <CommitList commits={p.items} base={base} />
              <Pagination page={page} next={p.next} prev={p.prev} last={p.last}
                onPage={(n) => { sp.set("page", String(n)); setSp(sp); window.scrollTo(0, 0); }} />
            </>
          )
        }
      </Async>
    </div>
  );
}

/** `/{o}/{r}/commit/{sha}` */
export function CommitPage() {
  const { repo, owner, name, base } = useRepo();
  const { sha = "" } = useParams();
  useTitle(`Commit ${shortSha(sha)} · ${repo.full_name}`);
  const st = useAsync(() => api.commit(owner, name, sha), [owner, name, sha]);
  return (
    <Async state={st}>
      {(c) => {
        const body = restLines(c.commit.message);
        return (
          <div>
            <div className="box commit-header">
              <div className="box-header">
                <h2 className="h3 commit-headline">{firstLine(c.commit.message)}</h2>
                <Link className="btn btn-sm" to={`${base}/tree/${c.sha}`}>Browse files</Link>
              </div>
              {body && <pre className="commit-body">{body}</pre>}
              <div className="box-footer commit-meta">
                <span>
                  <Avatar user={c.author} size={20} />{" "}
                  {c.author ? <Link to={`/${c.author.login}`} className="user-link">{c.author.login}</Link> : <strong>{c.commit.author?.name}</strong>}{" "}
                  committed <RelTime iso={c.commit.committer?.date || c.commit.author?.date} />
                </span>
                <span className="spacer" />
                <span className="muted small">
                  {c.parents.length > 0 && (
                    <>
                      {c.parents.length} parent{c.parents.length > 1 ? "s" : ""}{" "}
                      {c.parents.map((p, i) => (
                        <span key={p.sha}>
                          {i > 0 && " + "}
                          <Link className="mono" to={`${base}/commit/${p.sha}`}>{shortSha(p.sha)}</Link>
                        </span>
                      ))}{" "}
                    </>
                  )}
                  commit <span className="mono">{c.sha}</span>
                </span>
                <CopyButton text={c.sha} label="Copy full SHA" />
              </div>
            </div>
            <DiffView files={c.files || []} />
          </div>
        );
      }}
    </Async>
  );
}

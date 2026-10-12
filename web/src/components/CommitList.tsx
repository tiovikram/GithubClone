import { Link } from "react-router-dom";
import type { Commit } from "../api";
import { firstLine, shortSha } from "../util";
import { Avatar, CopyButton, RelTime } from "./common";

function groupByDay(commits: Commit[]): [string, Commit[]][] {
  const groups = new Map<string, Commit[]>();
  for (const c of commits) {
    const d = c.commit.committer?.date || c.commit.author?.date || "";
    const day = d ? new Date(d).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "Unknown date";
    if (!groups.has(day)) groups.set(day, []);
    groups.get(day)!.push(c);
  }
  return [...groups];
}

export function CommitList({ commits, base }: { commits: Commit[]; base: string }) {
  return (
    <div className="commit-groups">
      {groupByDay(commits).map(([day, list]) => (
        <section key={day} className="commit-group">
          <h3 className="commit-day muted">Commits on {day}</h3>
          <ul className="box list">
            {list.map((c) => (
              <li key={c.sha} className="list-row commit-row">
                <div className="commit-main">
                  <Link className="commit-title" to={`${base}/commit/${c.sha}`}>
                    {firstLine(c.commit.message)}
                  </Link>
                  <div className="muted small">
                    <Avatar user={c.author} size={16} />{" "}
                    {c.author ? <Link to={`/${c.author.login}`} className="user-link">{c.author.login}</Link> : <strong>{c.commit.author?.name}</strong>}{" "}
                    committed <RelTime iso={c.commit.committer?.date || c.commit.author?.date} />
                  </div>
                </div>
                <div className="commit-actions">
                  <Link className="btn btn-sm mono" to={`${base}/commit/${c.sha}`}>{shortSha(c.sha)}</Link>
                  <CopyButton text={c.sha} label="Copy full SHA" />
                  <Link className="btn btn-sm" to={`${base}/tree/${c.sha}`} title="Browse the repository at this point in the history">
                    &lt;&gt;
                  </Link>
                </div>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

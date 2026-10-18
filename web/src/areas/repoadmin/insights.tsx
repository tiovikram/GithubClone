// Insights tab: Pulse, Contributors, Community standards, Commits, Code frequency, Punch card,
// Traffic, Forks, and a link to the dependency graph (owned by the security area).
//   /{o}/{r}/pulse, /graphs/contributors, /graphs/commit-activity, /graphs/code-frequency,
//   /graphs/punch-card, /graphs/traffic, /forks, /community
import { useMemo, useState, type ReactNode } from "react";
import { Link, NavLink } from "react-router-dom";
import { Async, Avatar, Blankslate, RelTime } from "../../components/common";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { ra, type TrafficCount } from "./api";
import { Pill } from "./ui";

const fmtWeek = (unix: number) => new Date(unix * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "2-digit" });
const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

export function InsightsLayout({ children, title }: { children: ReactNode; title: string }) {
  const ctx = useRepo();
  useTitle(`${title} · ${ctx.repo.full_name}`);
  const b = ctx.base;
  const link = (to: string, label: string, key: string) => (
    <NavLink key={key} to={`${b}/${to}`} end className={({ isActive }) => "menu-item" + (isActive ? " selected" : "")} data-testid={`insights-nav-${key}`}>{label}</NavLink>
  );
  return (
    <div className="settings-layout ra-insights">
      <nav className="settings-nav" aria-label="Insights">
        <div className="settings-nav-group">
          {link("pulse", "Pulse", "pulse")}
          {link("graphs/contributors", "Contributors", "contributors")}
          {link("community", "Community standards", "community")}
          {ctx.canPush && link("graphs/traffic", "Traffic", "traffic")}
          {link("graphs/commit-activity", "Commits", "commits")}
          {link("graphs/code-frequency", "Code frequency", "code-frequency")}
          {link("graphs/punch-card", "Punch card", "punch-card")}
          {link("network/dependencies", "Dependency graph", "dependencies")}
          {link("forks", "Forks", "forks")}
        </div>
      </nav>
      <div className="settings-main">{children}</div>
    </div>
  );
}

// ---------- charts ----------

interface Bar { label: string; value: number; tip?: string }

/** Single-series bar chart (thin bars, rounded data end, hover tooltip via <title>). */
function Bars({ data, height = 120, label, color = "var(--accent)", testid }: { data: Bar[]; height?: number; label: string; color?: string; testid?: string }) {
  const max = Math.max(1, ...data.map((d) => d.value));
  const w = 10, gap = 2;
  const width = data.length * (w + gap);
  return (
    <figure className="ra-chart" data-testid={testid}>
      <svg viewBox={`0 0 ${Math.max(width, 1)} ${height + 2}`} preserveAspectRatio="none" role="img" aria-label={label} className="ra-chart-svg" style={{ height }}>
        <line x1={0} x2={width} y1={height} y2={height} className="ra-axis" />
        {data.map((d, i) => {
          const h = (d.value / max) * (height - 4);
          return (
            <g key={i}>
              <rect x={i * (w + gap)} y={0} width={w} height={height} fill="transparent"><title>{d.tip || `${d.label}: ${d.value}`}</title></rect>
              {d.value > 0 && <rect x={i * (w + gap) + 1} y={height - h} width={w - 2} height={h} rx={2} fill={color} pointerEvents="none" />}
            </g>
          );
        })}
      </svg>
      <figcaption className="small muted ra-chart-axis"><span>{data[0]?.label}</span><span>max {max}</span><span>{data[data.length - 1]?.label}</span></figcaption>
    </figure>
  );
}

function DataTable({ head, rows }: { head: string[]; rows: (string | number)[][] }) {
  return (
    <details className="ra-table-details">
      <summary className="small">Show data table</summary>
      <div className="ra-table-wrap">
        <table className="ra-table">
          <thead><tr>{head.map((h) => <th key={h}>{h}</th>)}</tr></thead>
          <tbody>{rows.map((r, i) => <tr key={i}>{r.map((c, j) => <td key={j}>{c}</td>)}</tr>)}</tbody>
        </table>
      </div>
    </details>
  );
}

// ---------- pages ----------

export function PulsePage() {
  const ctx = useRepo();
  const [days, setDays] = useState(7);
  const since = useMemo(() => new Date(Date.now() - days * 86400000).toISOString(), [days]);
  const st = useAsync(async () => {
    const [pulls, issues, commits, part] = await Promise.all([
      ra.pulls(ctx.owner, ctx.name, "all").catch(() => []),
      ra.issues(ctx.owner, ctx.name, "all", since).catch(() => []),
      ra.commitsSince(ctx.owner, ctx.name, since).catch(() => []),
      ra.participation(ctx.owner, ctx.name).catch(() => ({ all: [], owner: [] })),
    ]);
    const t0 = Date.parse(since);
    const after = (s: string | null) => !!s && Date.parse(s) >= t0;
    const realIssues = issues.filter((i) => !i.pull_request);
    return {
      merged: pulls.filter((p) => after(p.merged_at)),
      openedPrs: pulls.filter((p) => after(p.created_at)),
      closedIssues: realIssues.filter((i) => i.state === "closed" && after(i.closed_at)),
      newIssues: realIssues.filter((i) => after(i.created_at)),
      commits,
      authors: new Set(commits.map((c) => c.author?.login || c.commit.author?.name)).size,
      part,
    };
  }, [ctx.owner, ctx.name, since]);
  return (
    <InsightsLayout title="Pulse">
      <div className="ra-head-row">
        <h2 className="h3">Pulse</h2>
        <select className="input input-sm input-auto" aria-label="Period" value={days} onChange={(e) => setDays(Number(e.target.value))}>
          <option value={1}>1 day</option><option value={3}>3 days</option><option value={7}>1 week</option><option value={30}>1 month</option>
        </select>
      </div>
      <Async state={st}>
        {(d) => (
          <div data-testid="pulse">
            <div className="ra-stats">
              <div className="box box-body ra-stat"><div className="ra-stat-n">{d.merged.length}</div><div className="small muted">Merged pull requests</div></div>
              <div className="box box-body ra-stat"><div className="ra-stat-n">{d.openedPrs.length}</div><div className="small muted">Opened pull requests</div></div>
              <div className="box box-body ra-stat"><div className="ra-stat-n">{d.closedIssues.length}</div><div className="small muted">Closed issues</div></div>
              <div className="box box-body ra-stat"><div className="ra-stat-n">{d.newIssues.length}</div><div className="small muted">New issues</div></div>
            </div>
            <p data-testid="pulse-commits">
              Excluding merges, <strong>{d.authors} {d.authors === 1 ? "author has" : "authors have"}</strong> pushed <strong>{d.commits.length} {d.commits.length === 1 ? "commit" : "commits"}</strong> to all branches in the last {days === 1 ? "day" : `${days} days`}.
            </p>
            {d.part.all.length > 0 && (
              <section className="box box-body">
                <h3 className="h5">Weekly commits, last year</h3>
                <Bars label="Commits per week, last 52 weeks" data={d.part.all.map((v, i) => ({ label: `${52 - i}w ago`, value: v, tip: `${52 - i} weeks ago: ${v} commits (${d.part.owner[i] ?? 0} by the owner)` }))} />
              </section>
            )}
            {d.merged.length > 0 && (
              <section className="box">
                <div className="box-header"><h3 className="h5">{d.merged.length} pull requests merged</h3></div>
                <ul className="ra-list">{d.merged.map((p) => <li key={p.number} className="ra-row"><Pill kind="info">Merged</Pill><Link to={`${ctx.base}/pull/${p.number}`}>{p.title}</Link> <span className="muted small">#{p.number}</span></li>)}</ul>
              </section>
            )}
            {d.newIssues.length > 0 && (
              <section className="box">
                <div className="box-header"><h3 className="h5">{d.newIssues.length} new issues</h3></div>
                <ul className="ra-list">{d.newIssues.map((i) => <li key={i.number} className="ra-row"><Pill kind={i.state === "open" ? "ok" : ""}>{i.state}</Pill><Link to={`${ctx.base}/issues/${i.number}`}>{i.title}</Link> <span className="muted small">#{i.number}</span></li>)}</ul>
              </section>
            )}
          </div>
        )}
      </Async>
    </InsightsLayout>
  );
}

export function ContributorsPage() {
  const ctx = useRepo();
  const st = useAsync(() => ra.contributors(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  return (
    <InsightsLayout title="Contributors">
      <h2 className="h3">Contributors</h2>
      <Async state={st}>
        {(list) => {
          if (!list.length) return <div className="box"><Blankslate title="No contributions yet" /></div>;
          const weeks = list[0].weeks.map((w, i) => ({ w: w.w, c: list.reduce((n, s) => n + (s.weeks[i]?.c || 0), 0) }));
          const sorted = [...list].sort((a, b) => b.total - a.total);
          return (
            <div data-testid="contributors">
              <section className="box box-body">
                <h3 className="h5">Commits over time</h3>
                <Bars label="Commits per week, all contributors" data={weeks.slice(-52).map((w) => ({ label: fmtWeek(w.w), value: w.c }))} testid="contrib-chart" />
              </section>
              <ol className="ra-contrib-grid">
                {sorted.map((s, i) => {
                  const add = s.weeks.reduce((n, w) => n + w.a, 0), del = s.weeks.reduce((n, w) => n + w.d, 0);
                  return (
                    <li key={s.author?.login || i} className="box box-body" data-testid={`contributor-${s.author?.login || "ghost"}`}>
                      <div className="ra-contrib-head">
                        <Avatar user={s.author} size={32} />
                        <div className="grow">
                          {s.author ? <Link className="ra-strong" to={`/${s.author.login}`}>{s.author.login}</Link> : <span>ghost</span>}
                          <div className="small muted">{s.total} commits · <span className="c-add">{add} ++</span> · <span className="c-del">{del} --</span></div>
                        </div>
                        <span className="muted small">#{i + 1}</span>
                      </div>
                      <Bars height={50} label={`Weekly commits by ${s.author?.login}`} data={s.weeks.slice(-52).map((w) => ({ label: fmtWeek(w.w), value: w.c }))} />
                    </li>
                  );
                })}
              </ol>
            </div>
          );
        }}
      </Async>
    </InsightsLayout>
  );
}

export function CommitActivityPage() {
  const ctx = useRepo();
  const st = useAsync(() => ra.commitActivity(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const [sel, setSel] = useState<number | null>(null);
  return (
    <InsightsLayout title="Commit activity">
      <h2 className="h3">Commits</h2>
      <Async state={st}>
        {(weeks) => {
          if (!weeks.length) return <div className="box"><Blankslate title="No commits in the last year" /></div>;
          const idx = sel ?? weeks.length - 1;
          const wk = weeks[idx];
          return (
            <div data-testid="commit-activity">
              <section className="box box-body">
                <p className="small muted">{weeks.reduce((n, w) => n + w.total, 0)} commits in the last year</p>
                <Bars label="Commits per week, last year" data={weeks.map((w) => ({ label: fmtWeek(w.week), value: w.total }))} />
                <label className="small" htmlFor="ca-week">Week</label>{" "}
                <select id="ca-week" className="input input-sm input-auto" value={idx} onChange={(e) => setSel(Number(e.target.value))}>
                  {weeks.map((w, i) => <option key={w.week} value={i}>{fmtWeek(w.week)} ({w.total})</option>)}
                </select>
              </section>
              <section className="box box-body">
                <h3 className="h5">Week of {fmtWeek(wk.week)}</h3>
                <Bars height={80} label="Commits per day of the selected week" data={wk.days.map((v, i) => ({ label: DAYS[i].slice(0, 3), value: v, tip: `${DAYS[i]}: ${v}` }))} />
              </section>
              <DataTable head={["Week", "Commits"]} rows={weeks.filter((w) => w.total).map((w) => [fmtWeek(w.week), w.total])} />
            </div>
          );
        }}
      </Async>
    </InsightsLayout>
  );
}

export function CodeFrequencyPage() {
  const ctx = useRepo();
  const st = useAsync(() => ra.codeFrequency(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  return (
    <InsightsLayout title="Code frequency">
      <h2 className="h3">Code frequency</h2>
      <Async state={st}>
        {(rows) => {
          if (!rows.length) return <div className="box"><Blankslate title="No code changes yet" /></div>;
          const max = Math.max(1, ...rows.map((r) => Math.max(r[1], -r[2])));
          const h = 80, w = 10, gap = 2, width = rows.length * (w + gap);
          return (
            <div data-testid="code-frequency">
              <section className="box box-body">
                <div className="ra-legend small"><span className="ra-swatch" style={{ background: "var(--success)" }} /> Additions <span className="ra-swatch" style={{ background: "var(--danger)" }} /> Deletions</div>
                <svg viewBox={`0 0 ${width} ${h * 2}`} preserveAspectRatio="none" role="img" aria-label="Additions and deletions per week" className="ra-chart-svg" style={{ height: 160 }}>
                  <line x1={0} x2={width} y1={h} y2={h} className="ra-axis" />
                  {rows.map(([t, a, d], i) => (
                    <g key={t}>
                      <title>{`${fmtWeek(t)}: +${a} / ${d}`}</title>
                      {a > 0 && <rect x={i * (w + gap) + 1} y={h - (a / max) * (h - 2)} width={w - 2} height={(a / max) * (h - 2)} rx={2} fill="var(--success)" />}
                      {d < 0 && <rect x={i * (w + gap) + 1} y={h} width={w - 2} height={(-d / max) * (h - 2)} rx={2} fill="var(--danger)" />}
                    </g>
                  ))}
                </svg>
              </section>
              <DataTable head={["Week", "Additions", "Deletions"]} rows={rows.filter((r) => r[1] || r[2]).map((r) => [fmtWeek(r[0]), r[1], r[2]])} />
            </div>
          );
        }}
      </Async>
    </InsightsLayout>
  );
}

export function PunchCardPage() {
  const ctx = useRepo();
  const st = useAsync(() => ra.punchCard(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  return (
    <InsightsLayout title="Punch card">
      <h2 className="h3">Punch card</h2>
      <Async state={st}>
        {(rows) => {
          const max = Math.max(1, ...rows.map((r) => r[2]));
          return (
            <section className="box box-body" data-testid="punch-card">
              <p className="small muted">Commits by day of the week and hour (UTC). Bigger dots mean more commits.</p>
              <div className="ra-table-wrap">
                <svg viewBox="0 0 520 190" className="ra-punch" role="img" aria-label="Commits per day and hour">
                  {DAYS.map((d, i) => <text key={d} x={0} y={i * 24 + 20} className="ra-svg-text">{d.slice(0, 3)}</text>)}
                  {Array.from({ length: 24 }, (_, hr) => hr % 3 === 0 && <text key={hr} x={40 + hr * 20} y={185} className="ra-svg-text">{hr}</text>)}
                  {rows.map(([d, hr, n]) => n > 0 && (
                    <circle key={`${d}-${hr}`} cx={44 + hr * 20} cy={d * 24 + 16} r={2 + (n / max) * 8} fill="var(--accent)"><title>{`${DAYS[d]} ${hr}:00 — ${n} commits`}</title></circle>
                  ))}
                </svg>
              </div>
            </section>
          );
        }}
      </Async>
    </InsightsLayout>
  );
}

function TrafficChart({ title, data, kind, testid }: { title: string; data: TrafficCount; kind: "views" | "clones"; testid: string }) {
  const series = (data[kind] || []) as { timestamp: string; count: number; uniques: number }[];
  return (
    <section className="box box-body" data-testid={testid}>
      <div className="ra-head-row">
        <h3 className="h5">{title}</h3>
        <span className="small"><strong>{data.count}</strong> {kind === "views" ? "views" : "clones"} · <strong>{data.uniques}</strong> unique {kind === "views" ? "visitors" : "cloners"}</span>
      </div>
      {series.length ? (
        <Bars label={`${title} per day`} data={series.map((p) => ({ label: p.timestamp.slice(5, 10), value: p.count, tip: `${p.timestamp.slice(0, 10)}: ${p.count} (${p.uniques} unique)` }))} />
      ) : <p className="muted small">No {kind} in the last 14 days.</p>}
    </section>
  );
}

export function TrafficPage() {
  const ctx = useRepo();
  const st = useAsync(async () => {
    const [views, clones, refs, paths] = await Promise.all([ra.views(ctx.owner, ctx.name), ra.clones(ctx.owner, ctx.name), ra.referrers(ctx.owner, ctx.name), ra.paths(ctx.owner, ctx.name)]);
    return { views, clones, refs, paths };
  }, [ctx.owner, ctx.name]);
  if (!ctx.canPush) return <InsightsLayout title="Traffic"><Blankslate title="Traffic is only visible to people with push access" /></InsightsLayout>;
  return (
    <InsightsLayout title="Traffic">
      <h2 className="h3">Traffic</h2>
      <Async state={st}>
        {(d) => (
          <div data-testid="traffic">
            <TrafficChart title="Git clones" data={d.clones} kind="clones" testid="traffic-clones" />
            <TrafficChart title="Visitors" data={d.views} kind="views" testid="traffic-views" />
            <div className="ra-two-col">
              <section className="box">
                <div className="box-header"><h3 className="h5">Referring sites</h3></div>
                <table className="ra-table" data-testid="traffic-referrers">
                  <thead><tr><th>Site</th><th>Views</th><th>Unique visitors</th></tr></thead>
                  <tbody>{d.refs.length ? d.refs.map((r) => <tr key={r.referrer}><td>{r.referrer}</td><td>{r.count}</td><td>{r.uniques}</td></tr>) : <tr><td colSpan={3} className="muted">No referrers yet</td></tr>}</tbody>
                </table>
              </section>
              <section className="box">
                <div className="box-header"><h3 className="h5">Popular content</h3></div>
                <table className="ra-table" data-testid="traffic-paths">
                  <thead><tr><th>Content</th><th>Views</th><th>Unique visitors</th></tr></thead>
                  <tbody>{d.paths.length ? d.paths.map((p) => <tr key={p.path}><td className="ra-break"><Link to={p.path}>{p.path}</Link></td><td>{p.count}</td><td>{p.uniques}</td></tr>) : <tr><td colSpan={3} className="muted">No content views yet</td></tr>}</tbody>
                </table>
              </section>
            </div>
          </div>
        )}
      </Async>
    </InsightsLayout>
  );
}

export function ForksPage() {
  const ctx = useRepo();
  const [sort, setSort] = useState("newest");
  const st = useAsync(() => ra.forks(ctx.owner, ctx.name, sort), [ctx.owner, ctx.name, sort]);
  return (
    <InsightsLayout title="Forks">
      <div className="ra-head-row">
        <h2 className="h3">Forks</h2>
        <select className="input input-sm input-auto" aria-label="Sort forks" value={sort} onChange={(e) => setSort(e.target.value)}>
          <option value="newest">Newest</option><option value="oldest">Oldest</option><option value="stargazers">Most starred</option><option value="watchers">Most watched</option>
        </select>
      </div>
      <Async state={st}>
        {(list) => list.length === 0 ? <div className="box"><Blankslate title="No one has forked this repository yet">Forks are a great way to contribute to a repository.</Blankslate></div> : (
          <ul className="box ra-list" data-testid="forks">
            {list.map((f) => (
              <li key={f.id} className="ra-row">
                <Avatar user={f.owner} size={24} />
                <div className="ra-row-main">
                  <Link className="ra-strong" to={`/${f.full_name}`}>{f.full_name}</Link>
                  <div className="small muted">★ {f.stargazers_count} · forks {f.forks_count} · updated <RelTime iso={f.updated_at} /></div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </InsightsLayout>
  );
}

const COMMUNITY: [string, string, string][] = [
  ["description", "Description", ""],
  ["readme", "README", "README.md"],
  ["code_of_conduct", "Code of conduct", "CODE_OF_CONDUCT.md"],
  ["contributing", "Contributing", "CONTRIBUTING.md"],
  ["license", "License", "LICENSE"],
  ["issue_template", "Issue templates", ".github/ISSUE_TEMPLATE/bug_report.md"],
  ["pull_request_template", "Pull request template", ".github/pull_request_template.md"],
];

export function CommunityPage() {
  const ctx = useRepo();
  const st = useAsync(() => ra.community(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  return (
    <InsightsLayout title="Community standards">
      <h2 className="h3">Community standards</h2>
      <Async state={st}>
        {(p) => (
          <div data-testid="community">
            <div className="box box-body">
              <div className="ra-progress" role="progressbar" aria-valuenow={p.health_percentage} aria-valuemin={0} aria-valuemax={100} aria-label="Health">
                <span style={{ width: `${p.health_percentage}%` }} />
              </div>
              <p className="small muted">{p.health_percentage}% of the recommended community standards are met.</p>
            </div>
            <ul className="box ra-list" data-testid="community-checklist">
              {COMMUNITY.map(([k, label, path]) => {
                const ok = k === "description" ? !!p.description : !!p.files?.[k];
                const f = p.files?.[k];
                return (
                  <li key={k} className="ra-row" data-testid={`community-${k}`}>
                    <span className={ok ? "c-open" : "muted"} aria-hidden="true">{ok ? "✔" : "○"}</span>
                    <div className="ra-row-main">{label} {ok ? <span className="sr-only">(added)</span> : <span className="sr-only">(missing)</span>}</div>
                    <div className="ra-row-actions">
                      {ok ? (f?.html_url ? <a className="small" href={new URL(f.html_url, location.origin).pathname}>View</a> : <Pill kind="ok">Added</Pill>)
                        : k === "description" ? (ctx.isAdmin ? <Link className="btn btn-sm" to={`${ctx.base}/settings`}>Add</Link> : null)
                        : <span className="small muted">Add <code>{path}</code></span>}
                    </div>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </Async>
    </InsightsLayout>
  );
}


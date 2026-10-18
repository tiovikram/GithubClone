// /advisories and /advisories/GHSA-…: the advisory database browser.
import { useState, type FormEvent } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { Async, NotFoundView, Pagination, RelTime } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { ECOSYSTEMS, humanize, sec, ecoLabel } from "./api";
import { FilterSelect, PageHead, SeverityLabel } from "./ui";

/** Turn the search box into API filters: GHSA / CVE ids, `pkg@version`, or a package name. */
function searchQuery(q: string): Record<string, string> {
  const t = q.trim();
  if (!t) return {};
  if (/^GHSA-/i.test(t)) return { ghsa_id: t };
  if (/^CVE-/i.test(t)) return { cve_id: t.toUpperCase() };
  if (/^CWE-?\d+$/i.test(t)) return { cwes: t.replace(/^CWE-?/i, "") };
  return { affects: t };
}

export function GlobalAdvisoriesPage() {
  useTitle("GitHub Advisory Database");
  const [sp, setSp] = useSearchParams();
  const query = sp.get("query") || "";
  const type = sp.get("type") || "reviewed";
  const ecosystem = sp.get("ecosystem") || "";
  const severity = sp.get("severity") || "";
  const page = Number(sp.get("page") || 1);
  const [input, setInput] = useState(query);
  const set = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v);
    else n.delete(k);
    if (k !== "page") n.delete("page");
    setSp(n, { replace: true });
  };
  const list = useAsync(() => sec.globalAdvisories({ ...searchQuery(query), type, ecosystem, severity, page, sort: "published" }),
    [query, type, ecosystem, severity, page]);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    set("query", input);
  };
  return (
    <div className="container sec-global" data-testid="global-advisories">
      <PageHead title="GitHub Advisory Database" sub="Security vulnerability database inclusive of CVEs and GitHub originated security advisories from the world of open source software." />
      <form className="search-form" onSubmit={submit} role="search">
        <input className="input" aria-label="Search advisories" placeholder="Search by package (lodash, lodash@4.17.15), GHSA, CVE or CWE id" value={input}
          onChange={(e) => setInput(e.target.value)} data-testid="advisory-search" />
        <button type="submit" className="btn">Search</button>
      </form>
      <div className="sec-global-layout">
        <nav className="settings-nav sec-global-nav" aria-label="Advisory type">
          {[["reviewed", "GitHub reviewed"], ["unreviewed", "Unreviewed"], ["malware", "Malware"]].map(([k, l]) => (
            <button key={k} type="button" className={`menu-item${type === k ? " selected" : ""}`} onClick={() => set("type", k)} data-testid={`advisory-type-${k}`}>{l}</button>
          ))}
          <div className="sec-filters sec-mt">
            <FilterSelect id="ga-eco" label="Ecosystem" value={ecosystem} onChange={(v) => set("ecosystem", v)} options={ECOSYSTEMS.filter(([k]) => k !== "other")} />
            <FilterSelect id="ga-sev" label="Severity" value={severity} onChange={(v) => set("severity", v)}
              options={[["critical", "Critical"], ["high", "High"], ["medium", "Moderate"], ["low", "Low"]]} />
          </div>
        </nav>
        <div className="sec-global-main">
          <div className="box">
            <Async state={list}>
              {(p) => p.items.length === 0 ? (
                <div className="blankslate" data-testid="advisories-empty">
                  <h3>No advisories found</h3>
                  <p className="muted">Try a different search or filter. Site administrators import advisories with <code>forge advisories sync</code>.</p>
                </div>
              ) : (
                <ul className="list">
                  {p.items.map((a) => (
                    <li key={a.ghsa_id} className="list-row sec-alert-row" data-testid="global-advisory-row">
                      <div className="grow sec-row-main">
                        <Link className="issue-title" to={`/advisories/${a.ghsa_id}`}>{a.summary}</Link>
                        {" "}<SeverityLabel severity={a.severity} />
                        <div className="muted small sec-row-meta">
                          <span className="mono">{a.ghsa_id}</span>
                          {a.cve_id && <span className="mono">{a.cve_id}</span>}
                          published <RelTime iso={a.published_at} />
                          {a.vulnerabilities[0] && <> · {ecoLabel(a.vulnerabilities[0].package.ecosystem)} <strong className="mono">{a.vulnerabilities[0].package.name}</strong></>}
                          {a.withdrawn_at && <span className="badge badge-removed">Withdrawn</span>}
                        </div>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </Async>
          </div>
          {list.data && <Pagination page={page} next={list.data.next} prev={list.data.prev} last={list.data.last} onPage={(n) => set("page", String(n))} />}
        </div>
      </div>
    </div>
  );
}

export function GlobalAdvisoryPage() {
  const { ghsa = "" } = useParams();
  const st = useAsync(() => sec.globalAdvisory(ghsa), [ghsa]);
  useTitle(ghsa);
  return (
    <div className="container sec-global" data-testid="global-advisory">
      <div className="breadcrumbs small"><Link to="/advisories">GitHub Advisory Database</Link> / <span className="mono">{ghsa}</span></div>
      <Async state={st} notFound={<NotFoundView />}>
        {(a) => (
          <>
            <div className="sec-adv-head">
              <h2 className="sec-title" data-testid="advisory-title">{a.summary}</h2>
              <div className="sec-row-meta">
                <SeverityLabel severity={a.severity} />
                <span className="badge">{a.type === "reviewed" ? "GitHub Reviewed" : humanize(a.type)}</span>
                <span className="muted small">Published <RelTime iso={a.published_at} /> · Updated <RelTime iso={a.updated_at} /></span>
                {a.withdrawn_at && <span className="badge badge-removed">Withdrawn <RelTime iso={a.withdrawn_at} /></span>}
              </div>
            </div>
            <div className="layout-sidebar">
              <div className="layout-main">
                <div className="box">
                  <div className="box-header"><strong>Affected packages</strong></div>
                  <div className="sec-table-wrap">
                    <table className="sec-table">
                      <thead><tr><th>Package</th><th>Affected versions</th><th>Patched versions</th></tr></thead>
                      <tbody>
                        {a.vulnerabilities.map((v, i) => (
                          <tr key={i} data-testid="affected-package">
                            <td><span className="muted small">{ecoLabel(v.package.ecosystem)}</span><br /><strong className="mono">{v.package.name}</strong></td>
                            <td><code>{v.vulnerable_version_range || "—"}</code></td>
                            <td><code>{v.first_patched_version || "None"}</code></td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </div>
                <div className="box">
                  <div className="box-header"><strong>Description</strong></div>
                  <div className="box-body">{a.description ? <Markdown source={a.description} /> : <p className="muted">No description.</p>}</div>
                </div>
                {a.references.length > 0 && (
                  <div className="box">
                    <div className="box-header"><strong>References</strong></div>
                    <ul className="list">
                      {a.references.map((r) => <li key={r} className="list-row sec-wrap small"><a href={r} target="_blank" rel="noreferrer noopener">{r}</a></li>)}
                    </ul>
                  </div>
                )}
              </div>
              <aside className="layout-aside sec-aside">
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Severity</h3>
                  <SeverityLabel severity={a.severity} />
                  {a.cvss?.score != null && <div className="small sec-mt"><strong>{a.cvss.score.toFixed(1)}</strong> / 10<div className="mono small sec-wrap">{a.cvss.vector_string}</div></div>}
                </div>
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Weaknesses</h3>
                  {a.cwes.length ? a.cwes.map((c) => <div key={c.cwe_id} className="small"><span className="badge">{c.cwe_id}</span> {c.name}</div>) : <span className="muted small">No CWEs</span>}
                </div>
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Identifiers</h3>
                  {a.identifiers.map((i) => <div key={i.value} className="small mono">{i.value}</div>)}
                </div>
                {a.source_code_location && (
                  <div className="sidebar-section">
                    <h3 className="sidebar-heading">Source code</h3>
                    <a className="small sec-wrap" href={a.source_code_location} target="_blank" rel="noreferrer noopener">{a.source_code_location}</a>
                  </div>
                )}
              </aside>
            </div>
          </>
        )}
      </Async>
    </div>
  );
}

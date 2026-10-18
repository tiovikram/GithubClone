// /{o}/{r}/network/dependencies (dependency graph from the SBOM, grouped by ecosystem with
// SBOM export) and the dependency review box on pull requests.
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import type { Pull } from "../../api";
import { Async } from "../../components/common";
import { useRepo, type RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { sec, type SbomPackage } from "./api";
import { download, PageHead, SeverityLabel } from "./ui";

function parsePurl(p: SbomPackage) {
  const purl = p.externalRefs.find((r) => r.referenceType === "purl")?.referenceLocator || "";
  const m = purl.match(/^pkg:([^/]+)\/(.+?)(?:@([^?#]*))?(?:[?#].*)?$/);
  const [eco, name] = p.name.includes(":") ? [p.name.split(":")[0], p.name.slice(p.name.indexOf(":") + 1)] : [m?.[1] || "", p.name];
  return { ecosystem: eco, name: decodeURIComponent(name), version: p.versionInfo || m?.[3] || "", purl };
}

export function DependencyGraphPage() {
  const ctx = useRepo();
  useTitle(`Dependencies · ${ctx.repo.full_name}`);
  const sbom = useAsync(() => sec.sbom(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const alerts = useAsync(() => (ctx.canPush ? sec.dependabotAlerts(ctx.owner, ctx.name, { state: "open", per_page: 100 }).then((p) => p.items).catch(() => []) : Promise.resolve([])), [ctx.owner, ctx.name]);
  const [filter, setFilter] = useState("");
  const deps = useMemo(() => (sbom.data?.sbom.packages || []).filter((p) => p.SPDXID !== "SPDXRef-Repository").map(parsePurl), [sbom.data]);
  const vulnerable = (name: string) => (alerts.data || []).filter((a) => a.dependency.package.name.toLowerCase() === name.toLowerCase());
  const groups = useMemo(() => {
    const m = new Map<string, typeof deps>();
    for (const d of deps.filter((d) => !filter || d.name.toLowerCase().includes(filter.toLowerCase()))) m.set(d.ecosystem, [...(m.get(d.ecosystem) || []), d]);
    return [...m.entries()].sort();
  }, [deps, filter]);
  return (
    <div className="sec-depgraph" data-testid="dependency-graph">
      <PageHead title="Dependency graph" sub={<>Dependencies of the default branch (<span className="mono">{ctx.repo.default_branch}</span>) from its manifest files and submitted snapshots.</>}>
        <button type="button" className="btn btn-sm" disabled={!sbom.data} data-testid="export-sbom"
          onClick={() => sbom.data && download(`${ctx.owner}_${ctx.name}.spdx.json`, JSON.stringify(sbom.data.sbom, null, 2))}>Export SBOM</button>
        {ctx.canPush && <Link className="btn btn-sm" to={`${ctx.base}/security/dependabot`}>Dependabot alerts</Link>}
      </PageHead>
      <div className="sec-filters">
        <input className="input input-sm sec-search" placeholder="Search all dependencies" aria-label="Search dependencies" value={filter}
          onChange={(e) => setFilter(e.target.value)} data-testid="dep-filter" />
      </div>
      <Async state={sbom}>
        {() => groups.length === 0 ? (
          <div className="blankslate" data-testid="no-deps">
            <h3>No dependencies found</h3>
            <p className="muted">Add a supported manifest (package.json, package-lock.json, Cargo.toml, Cargo.lock, go.mod, requirements.txt) or submit a dependency snapshot.</p>
          </div>
        ) : (
          <>
            {groups.map(([eco, list]) => (
              <div key={eco} className="box" data-testid="dep-group">
                <div className="box-header"><strong>{eco}</strong> <span className="counter">{list.length}</span></div>
                <ul className="list">
                  {list.map((d) => {
                    const v = vulnerable(d.name);
                    return (
                      <li key={d.purl || d.name + d.version} className="list-row sec-dep-row" data-testid="dep-row">
                        <span className="grow mono sec-wrap">{d.name}</span>
                        <span className="mono small muted">{d.version || "—"}</span>
                        {v.length > 0 && (
                          <Link to={`${ctx.base}/security/dependabot/${v[0].number}`} className="sec-dep-vuln" data-testid="dep-vuln">
                            <SeverityLabel severity={v[0].security_vulnerability.severity} /> {v.length} alert{v.length === 1 ? "" : "s"}
                          </Link>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </div>
            ))}
            <p className="muted small">{deps.length} dependenc{deps.length === 1 ? "y" : "ies"} · SBOM generated {sbom.data && new Date(sbom.data.sbom.creationInfo.created).toLocaleString()}</p>
          </>
        )}
      </Async>
    </div>
  );
}

/** Slot `pull.merge.top`: added / removed dependencies and their vulnerabilities. */
export function DependencyReview({ ctx, pull }: { ctx: RepoCtx; pull: Pull }) {
  const changes = useAsync(
    () => sec.depCompare(ctx.owner, ctx.name, pull.base.sha || pull.base.ref, pull.head.sha || pull.head.ref).catch(() => null),
    [ctx.owner, ctx.name, pull.base.sha, pull.head.sha],
  );
  const [open, setOpen] = useState(false);
  const list = changes.data;
  if (!list || list.length === 0) return null;
  const added = list.filter((c) => c.change_type === "added");
  const removed = list.filter((c) => c.change_type === "removed");
  const vulns = added.flatMap((c) => c.vulnerabilities.map((v) => ({ ...v, dep: c })));
  return (
    <div className={`box sec-depreview ${vulns.length ? "sec-depreview-bad" : ""}`} data-testid="dependency-review">
      <div className="box-header">
        <strong>Dependency review</strong>
        <span className="muted small">{added.length} added, {removed.length} removed{vulns.length ? ` · ${vulns.length} vulnerabilit${vulns.length === 1 ? "y" : "ies"}` : " · no known vulnerabilities"}</span>
        <span className="grow" />
        <button type="button" className="btn btn-sm" onClick={() => setOpen(!open)} aria-expanded={open} data-testid="dependency-review-toggle">{open ? "Hide" : "Show"} changes</button>
      </div>
      {vulns.length > 0 && (
        <ul className="list">
          {vulns.map((v) => (
            <li key={v.advisory_ghsa_id + v.dep.manifest + v.dep.name} className="list-row" data-testid="dependency-review-vuln">
              <SeverityLabel severity={v.severity} />
              <span className="grow"><span className="mono">{v.dep.name}@{v.dep.version}</span> — <Link to={`/advisories/${v.advisory_ghsa_id}`}>{v.advisory_summary}</Link></span>
              <span className="muted small mono">{v.dep.manifest}</span>
            </li>
          ))}
        </ul>
      )}
      {open && (
        <ul className="list sec-depreview-list">
          {list.map((c) => (
            <li key={c.change_type + c.manifest + c.name + c.version} className="list-row" data-testid="dependency-review-change">
              <span className={`badge ${c.change_type === "added" ? "badge-added" : "badge-removed"}`}>{c.change_type === "added" ? "+" : "−"}</span>
              <span className="grow mono sec-wrap">{c.name} {c.version}</span>
              <span className="muted small">{c.ecosystem} · {c.scope}</span>
              <span className="muted small mono sec-wrap">{c.manifest}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

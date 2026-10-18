// Enterprise audit log (search, filter, export) and live migrations.
import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import type { AdminCtx } from "../types";
import { Async, Blankslate, Pagination, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { fullDate } from "../../util";
import { adminApi, type AuditEvent, type LiveMigration } from "./api";
import { download, PageHead, Pill, useAction } from "./ui";

const QUICK: [string, string][] = [
  ["", "All events"],
  ["action:user", "Users"],
  ["action:org", "Organizations"],
  ["action:business", "Enterprise settings"],
  ["action:hook", "Webhooks"],
  ["action:pre_receive_hook", "Pre-receive hooks"],
  ["action:oauth_access", "Tokens"],
];

export function AuditLogSection({ ctx }: { ctx: AdminCtx }) {
  const [phrase, setPhrase] = useState("");
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(1);
  const log = useAsync(() => adminApi.auditLog(ctx.enterprise, query, page), [ctx.enterprise, query, page]);
  const act = useAction();
  const search = (e: FormEvent) => { e.preventDefault(); setPage(1); setQuery(phrase.trim()); };
  const exportAs = (fmt: "json" | "csv") => act.run(async () => {
    // Export every matching event (up to 10 pages of 100).
    const all: AuditEvent[] = [];
    for (let p = 1; p <= 10; p++) {
      const r = await adminApi.auditLog(ctx.enterprise, query, p, 100);
      all.push(...r.items);
      if (!r.next) break;
    }
    const stamp = new Date().toISOString().slice(0, 10);
    if (fmt === "json") download(`audit-log-${stamp}.json`, JSON.stringify(all, null, 2), "application/json");
    else {
      const cols = ["created_at", "action", "actor", "user", "org", "repo", "team", "actor_ip", "data"] as const;
      const esc = (v: unknown) => { const s = v === undefined || v === null ? "" : typeof v === "object" ? JSON.stringify(v) : String(v); return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s; };
      const rows = all.map((e) => cols.map((c) => esc(c === "created_at" ? new Date(e.created_at).toISOString() : e[c])).join(","));
      download(`audit-log-${stamp}.csv`, [cols.join(","), ...rows].join("\n"), "text/csv");
    }
  }, "Export downloaded.");
  return (
    <div data-testid="sa-audit">
      <PageHead title="Audit log" desc={<>Instance and organization events. Filter with qualifiers: <code>action:</code>, <code>actor:</code>, <code>user:</code>, <code>org:</code>, <code>repo:</code>, <code>team:</code>, <code>created:&gt;=YYYY-MM-DD</code>.</>}>
        <button type="button" className="btn btn-sm" disabled={act.busy} onClick={() => exportAs("json")} data-testid="sa-audit-export-json">Export JSON</button>
        <button type="button" className="btn btn-sm" disabled={act.busy} onClick={() => exportAs("csv")} data-testid="sa-audit-export-csv">Export CSV</button>
      </PageHead>
      {act.status}
      <form className="sa-filter" role="search" onSubmit={search}>
        <label htmlFor="sa-audit-q" className="sr-only">Search audit log</label>
        <input id="sa-audit-q" className="input mono" placeholder="action:user.suspend actor:octocat" value={phrase} onChange={(e) => setPhrase(e.target.value)} />
        <label htmlFor="sa-audit-quick" className="sr-only">Quick filter</label>
        <select id="sa-audit-quick" className="input input-auto" value={QUICK.some(([q]) => q === query) ? query : "custom"}
          onChange={(e) => { if (e.target.value !== "custom") { setPhrase(e.target.value); setQuery(e.target.value); setPage(1); } }}>
          {QUICK.map(([q, label]) => <option key={q} value={q}>{label}</option>)}
          {!QUICK.some(([q]) => q === query) && <option value="custom">Custom search</option>}
        </select>
        <button type="submit" className="btn">Search</button>
      </form>
      <Async state={log}>
        {(p) => p.items.length === 0 ? <Blankslate title="No events match" /> : (
          <>
            <ul className="box list" aria-label="Audit log events">
              {p.items.map((ev) => (
                <li key={ev._document_id} className="list-row sa-row sa-audit-row" data-testid="sa-audit-event">
                  <div className="grow sa-row-main">
                    <div className="sa-row-title"><code className="sa-action-name">{ev.action}</code>
                      {ev.actor && <span>by <Link to={`/${ev.actor}`}>{ev.actor}</Link></span>}</div>
                    <div className="muted small">
                      {[ev.user && `user ${ev.user}`, ev.org && `org ${ev.org}`, ev.repo && `repo ${ev.repo}`, ev.team && `team ${ev.team}`, ev.actor_ip && `from ${ev.actor_ip}`].filter(Boolean).join(" · ")}
                      {ev.data && typeof ev.data.reason === "string" && ev.data.reason ? ` · reason “${ev.data.reason}”` : ""}
                    </div>
                  </div>
                  <span className="muted small" title={fullDate(new Date(ev.created_at).toISOString())}><RelTime iso={new Date(ev.created_at).toISOString()} /></span>
                </li>
              ))}
            </ul>
            <Pagination page={page} next={p.next} prev={p.prev} last={p.last} onPage={setPage} />
          </>
        )}
      </Async>
    </div>
  );
}

const ACTIONS: Record<string, string[]> = {
  created: ["start", "pause", "cancel"],
  in_progress: ["pause", "cancel", "cutover"],
  paused: ["resume", "cancel"],
  failed: ["cutover", "revert-cutover"],
  completed: ["revert-cutover"],
};

export function MigrationsSection() {
  const list = useAsync(() => adminApi.migrations(), []);
  const [form, setForm] = useState<Record<string, string>>({ target_api_endpoint: "https://", target_visibility: "private" });
  const act = useAction();
  const f = (k: string, label: string, ph?: string) => (
    <div className="form-group">
      <label htmlFor={`sa-mig-${k}`}>{label}</label>
      <input id={`sa-mig-${k}`} className="input" required placeholder={ph} value={form[k] || ""} onChange={(e) => setForm({ ...form, [k]: e.target.value })} />
    </div>
  );
  return (
    <div data-testid="sa-migrations">
      <PageHead title="Live migrations" desc="Migrate repositories to another GitHub Enterprise instance. forge records migrations and their state; it cannot transfer the data, so starting a migration reports a failure." />
      {act.status}
      <Async state={list}>
        {(l) => l.migrations.length === 0 ? <Blankslate title="No live migrations" /> : (
          <ul className="box list" aria-label="Live migrations">
            {l.migrations.map((m) => <MigrationRow key={m.migration_id} m={m} run={act.run} busy={act.busy} onChange={list.reload} />)}
          </ul>
        )}
      </Async>
      <form className="box box-body" aria-label="New live migration" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => adminApi.createMigration(form), "Migration created.")) list.reload(); }}>
        <h3 className="h5">New live migration</h3>
        <div className="form-row">
          {f("source_organization_login", "Source owner")}
          {f("source_repository_name", "Source repository")}
        </div>
        <div className="form-row">
          {f("target_organization_login", "Target organization")}
          {f("target_repository_name", "Target repository")}
        </div>
        <div className="form-row">
          {f("target_api_endpoint", "Target API endpoint", "https://ghe.example.com/api/v3")}
          {f("pat_name", "Token name on the target")}
          <div className="form-group">
            <label htmlFor="sa-mig-vis">Target visibility</label>
            <select id="sa-mig-vis" className="input input-auto" value={form.target_visibility} onChange={(e) => setForm({ ...form, target_visibility: e.target.value })}>
              <option value="private">private</option>
              <option value="internal">internal</option>
            </select>
          </div>
        </div>
        <button type="submit" className="btn btn-primary" disabled={act.busy}>Create migration</button>
      </form>
    </div>
  );
}

function MigrationRow({ m, run, busy, onChange }: { m: LiveMigration; run: (fn: () => Promise<unknown>, msg?: string) => Promise<boolean>; busy: boolean; onChange: () => void }) {
  return (
    <li className="list-row sa-row" data-testid={`sa-mig-${m.source_organization_login}/${m.source_repository_name}`}>
      <div className="grow sa-row-main">
        <div className="sa-row-title"><strong>{m.source_organization_login}/{m.source_repository_name}</strong> → <span>{m.target_organization_login}/{m.target_repository_name}</span><Pill>{m.status}</Pill></div>
        <div className="muted small sa-break">{m.target_api_endpoint} · created <RelTime iso={m.created_at} />{m.completed_at ? <> · finished <RelTime iso={m.completed_at} /></> : ""}</div>
      </div>
      <div className="sa-row-actions">
        {(ACTIONS[m.status] || []).map((a) => (
          <button key={a} type="button" className="btn btn-sm" disabled={busy}
            onClick={async () => { await run(() => adminApi.migrationAction(m.migration_id, a), `Migration ${a} requested.`); onChange(); }}>{a.replace("-", " ")}</button>
        ))}
      </div>
    </li>
  );
}

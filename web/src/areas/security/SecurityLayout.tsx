// /{o}/{r}/security/*: the Security tab with GitHub's left navigation.
import { useEffect, useState, type FormEvent } from "react";
import { Link, NavLink, Route, Routes, useNavigate } from "react-router-dom";
import { api, decodeBase64, isNotFound, request, type ContentItem } from "../../api";
import { Async, NotFoundView } from "../../components/common";
import { useRepo, type RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { sec } from "./api";
import { AdvisoriesPage, AdvisoryPage, NewAdvisoryPage } from "./Advisories";
import { CodeScanningAlertPage, CodeScanningPage, CodeScanningToolsPage } from "./CodeScanning";
import { DependabotAlertPage, DependabotPage } from "./Dependabot";
import { BypassRequestPage, BypassRequestsPage, SecretAlertPage, SecretScanningPage, UnblockSecretPage } from "./SecretScanning";
import { PageHead, ShieldIcon, useAction, InlineError } from "./ui";

/** Who may see which alert lists (mirrors the server's checks). */
export const access = (ctx: RepoCtx) => ({
  dependabot: ctx.canPush,
  code: ctx.canPush || !ctx.repo.private,
  secrets: ctx.isAdmin,
});

function useCounts(ctx: RepoCtx) {
  const a = access(ctx);
  return useAsync(async () => {
    const one = async (f: () => Promise<{ items: unknown[]; last?: number }>) => {
      try {
        const p = await f();
        return p.items.length;
      } catch {
        return undefined;
      }
    };
    const [dependabot, code, secrets] = await Promise.all([
      a.dependabot ? one(() => sec.dependabotAlerts(ctx.owner, ctx.name, { state: "open", per_page: 100 })) : undefined,
      a.code ? one(() => sec.codeAlerts(ctx.owner, ctx.name, { state: "open", per_page: 100 })) : undefined,
      a.secrets ? one(() => sec.secretAlerts(ctx.owner, ctx.name, { state: "open", per_page: 100 })) : undefined,
    ]);
    return { dependabot, code, secrets };
  }, [ctx.owner, ctx.name]);
}

export function SecurityLayout() {
  const ctx = useRepo();
  useTitle(`Security · ${ctx.repo.full_name}`);
  const counts = useCounts(ctx);
  const a = access(ctx);
  const root = `${ctx.base}/security`;
  const cls = ({ isActive }: { isActive: boolean }) => "menu-item" + (isActive ? " selected" : "");
  const n = (v: number | undefined) => (v !== undefined && v > 0 ? <span className="counter">{v}</span> : null);
  return (
    <div className="settings-layout sec-layout">
      <nav className="settings-nav sec-nav" aria-label="Security">
        <div className="settings-nav-group">
          <NavLink end to={root} className={cls} data-testid="sec-nav-overview"><ShieldIcon /> Overview</NavLink>
        </div>
        <div className="settings-nav-group">
          <h2 className="settings-nav-heading">Reporting</h2>
          <NavLink to={`${root}/policy`} className={cls} data-testid="sec-nav-policy">Policy</NavLink>
          <NavLink to={`${root}/advisories`} className={cls} data-testid="sec-nav-advisories">Advisories</NavLink>
        </div>
        {(a.dependabot || a.code || a.secrets) && (
          <div className="settings-nav-group">
            <h2 className="settings-nav-heading">Vulnerability alerts</h2>
            {a.dependabot && <NavLink to={`${root}/dependabot`} className={cls} data-testid="sec-nav-dependabot">Dependabot {n(counts.data?.dependabot)}</NavLink>}
            {a.code && <NavLink to={`${root}/code-scanning`} className={cls} data-testid="sec-nav-code-scanning">Code scanning {n(counts.data?.code)}</NavLink>}
            {a.secrets && <NavLink to={`${root}/secret-scanning`} className={cls} data-testid="sec-nav-secret-scanning">Secret scanning {n(counts.data?.secrets)}</NavLink>}
          </div>
        )}
        <div className="settings-nav-group">
          <h2 className="settings-nav-heading">Supply chain</h2>
          <NavLink to={`${ctx.base}/network/dependencies`} className={cls} data-testid="sec-nav-dependency-graph">Dependency graph</NavLink>
        </div>
      </nav>
      <div className="settings-main sec-main">
        <Routes>
          <Route index element={<Overview counts={counts.data} />} />
          <Route path="policy" element={<PolicyPage />} />
          <Route path="advisories" element={<AdvisoriesPage />} />
          <Route path="advisories/new" element={<NewAdvisoryPage />} />
          <Route path="advisories/:ghsa" element={<AdvisoryPage />} />
          <Route path="dependabot" element={<DependabotPage />} />
          <Route path="dependabot/:number" element={<DependabotAlertPage />} />
          <Route path="code-scanning" element={<CodeScanningPage />} />
          <Route path="code-scanning/tools" element={<CodeScanningToolsPage />} />
          <Route path="code-scanning/:number" element={<CodeScanningAlertPage />} />
          <Route path="secret-scanning" element={<SecretScanningPage />} />
          <Route path="secret-scanning/push-protection-bypasses" element={<BypassRequestsPage />} />
          <Route path="secret-scanning/push-protection-bypasses/:number" element={<BypassRequestPage />} />
          <Route path="secret-scanning/unblock-secret/:placeholder" element={<UnblockSecretPage />} />
          <Route path="secret-scanning/:number" element={<SecretAlertPage />} />
          <Route path="*" element={<NotFoundView />} />
        </Routes>
      </div>
    </div>
  );
}

// ---------- overview ----------

function Overview({ counts }: { counts?: { dependabot?: number; code?: number; secrets?: number } }) {
  const ctx = useRepo();
  const root = `${ctx.base}/security`;
  const policy = useAsync(() => findPolicy(ctx), [ctx.owner, ctx.name]);
  const advisories = useAsync(() => sec.advisories(ctx.owner, ctx.name).catch(() => []), [ctx.owner, ctx.name]);
  const pvr = useAsync(() => sec.pvr(ctx.owner, ctx.name).catch(() => ({ enabled: false })), [ctx.owner, ctx.name]);
  const repo = useAsync(() => (ctx.isAdmin ? sec.repo(ctx.owner, ctx.name) : Promise.resolve(undefined)), [ctx.owner, ctx.name]);
  const vuln = useAsync(() => (ctx.canPush ? sec.vulnAlerts(ctx.owner, ctx.name).catch(() => false) : Promise.resolve(false)), [ctx.owner, ctx.name]);
  const sa = repo.data?.security_and_analysis;
  const on = (k: keyof NonNullable<typeof sa>) => (sa?.[k] as { status?: string } | undefined)?.status === "enabled";
  const settings = `${ctx.base}/settings/security_analysis`;
  const row = (title: string, status: string, enabled: boolean | undefined, link: string, linkText: string, testid: string) => (
    <li className="list-row sec-overview-row" data-testid={`overview-${testid}`}>
      <span className={`sec-dot ${enabled ? "sec-dot-on" : ""}`} aria-hidden="true" />
      <div className="grow">
        <strong>{title}</strong>
        <div className="muted small">{status}</div>
      </div>
      <Link className="btn btn-sm" to={link}>{linkText}</Link>
    </li>
  );
  return (
    <div data-testid="security-overview">
      <PageHead title="Security overview" sub="Security and quality for this repository." />
      <div className="box">
        <div className="box-header"><h3>Security policy and reporting</h3></div>
        <ul className="list">
          {row("Security policy", policy.data ? `Defined in ${policy.data.path}` : policy.loading ? "Checking…" : "Disabled: define how users should report security vulnerabilities",
            !!policy.data, `${root}/policy`, policy.data ? "View security policy" : "Set up a security policy", "policy")}
          {row("Security advisories", `${(advisories.data || []).length} advisor${(advisories.data || []).length === 1 ? "y" : "ies"}`,
            (advisories.data || []).length > 0, `${root}/advisories`, "View security advisories", "advisories")}
          {row("Private vulnerability reporting", pvr.data?.enabled ? "Enabled: anyone can privately report a vulnerability" : "Disabled",
            pvr.data?.enabled, pvr.data?.enabled ? `${root}/advisories/new` : ctx.isAdmin ? settings : `${root}/advisories`,
            pvr.data?.enabled ? "Report a vulnerability" : ctx.isAdmin ? "Enable in settings" : "View advisories", "pvr")}
        </ul>
      </div>
      {(ctx.canPush || !ctx.repo.private) && (
        <div className="box">
          <div className="box-header"><h3>Vulnerability alerts</h3></div>
          <ul className="list">
            {ctx.canPush && row("Dependabot alerts", vuln.data ? `${counts?.dependabot ?? 0} open alert${counts?.dependabot === 1 ? "" : "s"}` : "Disabled: get notified when one of your dependencies has a vulnerability",
              vuln.data, `${root}/dependabot`, "View Dependabot alerts", "dependabot")}
            {row("Code scanning alerts", counts?.code !== undefined ? `${counts.code} open alert${counts.code === 1 ? "" : "s"}` : "Automatically detect common vulnerability and coding errors",
              (counts?.code ?? 0) > 0, `${root}/code-scanning`, "View code scanning", "code-scanning")}
            {ctx.isAdmin && row("Secret scanning alerts", on("secret_scanning") ? `${counts?.secrets ?? 0} open alert${counts?.secrets === 1 ? "" : "s"}${on("secret_scanning_push_protection") ? " · push protection on" : ""}` : "Disabled: get notified when a secret is pushed to this repository",
              on("secret_scanning"), `${root}/secret-scanning`, "View secret scanning", "secret-scanning")}
          </ul>
        </div>
      )}
      {ctx.isAdmin && (
        <p className="muted small">Configure these features in <Link to={settings} data-testid="overview-settings-link">Settings → Advanced Security</Link>.</p>
      )}
    </div>
  );
}

// ---------- security policy ----------

const POLICY_PATHS = ["SECURITY.md", ".github/SECURITY.md", "docs/SECURITY.md"];

export async function findPolicy(ctx: RepoCtx): Promise<{ path: string; text: string } | null> {
  for (const path of POLICY_PATHS) {
    try {
      const c = (await api.contents(ctx.owner, ctx.name, path)) as ContentItem;
      if (!Array.isArray(c) && c.type === "file") return { path, text: c.content ? decodeBase64(c.content) : await api.rawFile(ctx.owner, ctx.name, path) };
    } catch (e) {
      if (!isNotFound(e)) throw e;
    }
  }
  return null;
}

const POLICY_TEMPLATE = `# Security Policy

## Supported Versions

Use this section to tell people about which versions of your project are
currently being supported with security updates.

| Version | Supported          |
| ------- | ------------------ |
| 1.x     | :white_check_mark: |
| < 1.0   | :x:                |

## Reporting a Vulnerability

Use this section to tell people how to report a vulnerability.

Tell them where to go, how often they can expect to get an update on a
reported vulnerability, what to expect if the vulnerability is accepted or
declined, etc.
`;

function PolicyPage() {
  const ctx = useRepo();
  const state = useAsync(() => findPolicy(ctx), [ctx.owner, ctx.name]);
  const pvr = useAsync(() => sec.pvr(ctx.owner, ctx.name).catch(() => ({ enabled: false })), [ctx.owner, ctx.name]);
  const [editing, setEditing] = useState(false);
  return (
    <div data-testid="security-policy">
      <PageHead title="Security policy">
        {pvr.data?.enabled && <Link className="btn btn-sm btn-primary" to={`${ctx.base}/security/advisories/new`} data-testid="report-vuln-link">Report a vulnerability</Link>}
      </PageHead>
      <Async state={state}>
        {(p) => p && !editing ? (
          <div className="box">
            <div className="box-header">
              <strong className="mono">{p.path}</strong>
              <span className="grow" />
              <Link className="btn btn-sm" to={`${ctx.base}/blob/${ctx.repo.default_branch}/${p.path}`}>View file</Link>
              {ctx.canPush && <button type="button" className="btn btn-sm" onClick={() => setEditing(true)}>Edit policy</button>}
            </div>
            <div className="box-body"><Markdown source={p.text} repoPath={ctx.base} /></div>
          </div>
        ) : editing || !p ? (
          editing || (ctx.canPush && p === null) ? (
            <PolicyEditor existing={p} onDone={() => { setEditing(false); state.reload(); }} canPush={ctx.canPush} />
          ) : null
        ) : null}
      </Async>
      {state.data === null && !ctx.canPush && (
        <div className="blankslate" data-testid="no-policy">
          <h3>No security policy detected</h3>
          <p className="muted">This project has not set up a SECURITY.md file yet.</p>
        </div>
      )}
    </div>
  );
}

function PolicyEditor({ existing, onDone, canPush }: { existing: { path: string; text: string } | null; onDone: () => void; canPush: boolean }) {
  const ctx = useRepo();
  const nav = useNavigate();
  const [started, setStarted] = useState(!!existing);
  const [text, setText] = useState(existing?.text || POLICY_TEMPLATE);
  const [message, setMessage] = useState(existing ? "Update SECURITY.md" : "Create SECURITY.md");
  const act = useAction();
  useEffect(() => setText(existing?.text || POLICY_TEMPLATE), [existing]);
  if (!canPush) return null;
  if (!started) {
    return (
      <div className="blankslate" data-testid="no-policy">
        <div className="blankslate-icon"><ShieldIcon size={24} /></div>
        <h3>Set up a security policy</h3>
        <p className="muted">Help your community understand your security procedures by creating a SECURITY.md file in this repository.</p>
        <button type="button" className="btn btn-primary" onClick={() => setStarted(true)} data-testid="start-policy">Start setup</button>
      </div>
    );
  }
  const path = existing?.path || "SECURITY.md";
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const ok = await act.run(async () => {
      let sha: string | undefined;
      if (existing) {
        const c = (await api.contents(ctx.owner, ctx.name, path)) as ContentItem;
        sha = c.sha;
      }
      const bytes = new TextEncoder().encode(text);
      let bin = "";
      bytes.forEach((b) => (bin += String.fromCharCode(b)));
      await request(`/repos/${encodeURIComponent(ctx.owner)}/${encodeURIComponent(ctx.name)}/contents/${path}`, {
        method: "PUT", body: { message, content: btoa(bin), sha, branch: ctx.repo.default_branch },
      });
    });
    if (ok) {
      onDone();
      nav(`${ctx.base}/security/policy`);
    }
  };
  return (
    <form className="box box-body" onSubmit={submit} data-testid="policy-editor">
      <div className="form-group">
        <label htmlFor="policy-text">{path}</label>
        <textarea id="policy-text" className="input textarea mono sec-policy-text" rows={18} value={text} onChange={(e) => setText(e.target.value)} />
      </div>
      <div className="form-group">
        <label htmlFor="policy-message">Commit message</label>
        <input id="policy-message" className="input" value={message} onChange={(e) => setMessage(e.target.value)} />
      </div>
      <InlineError error={act.error} />
      <div className="form-actions">
        <button type="button" className="btn" onClick={onDone}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={act.busy} data-testid="commit-policy">
          {act.busy ? "Committing…" : `Commit to ${ctx.repo.default_branch}`}
        </button>
      </div>
    </form>
  );
}


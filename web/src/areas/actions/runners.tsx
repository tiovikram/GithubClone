// Self-hosted runners (repo / org / enterprise): list, labels, remove, "New self-hosted runner"
// with the registration token and `forge runner` command; runner groups (org / enterprise).
import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";
import { Async, Blankslate, CodeLine, ErrorBox, Flash } from "../../components/common";
import { useAsync } from "../../hooks";
import { origin } from "../../util";
import { actionsApi as A, type Runner, type RunnerGroup, type Scope } from "./api";
import { Dropdown } from "./ui";

const scopeWord = (s: Scope) => (s.kind === "org" ? "organization" : s.kind === "enterprise" ? "enterprise" : "repository");

export function RunnersList({ scope, root, groups }: { scope: Scope; root: string; groups?: boolean }) {
  const st = useAsync(() => A.runners(scope), [JSON.stringify(scope)]);
  const gs = useAsync(() => (groups ? A.runnerGroups(scope).then((g) => g.runner_groups) : Promise.resolve([] as RunnerGroup[])), [JSON.stringify(scope), groups]);
  const [err, setErr] = useState<unknown>();
  return (
    <>
      <div className="act-head">
        <h2 className="h3">Runners</h2>
        <Link className="btn btn-sm btn-primary" to={`${root}/new`} data-testid="new-runner">New self-hosted runner</Link>
      </div>
      <p className="muted">Host your own runners and customize the environment used to run jobs in your GitHub Actions workflows. Runners registered here are available to {scope.kind === "repo" ? "this repository" : scope.kind === "org" ? "repositories in this organization (through runner groups)" : "organizations in this enterprise (through runner groups)"}.</p>
      {err !== undefined && <ErrorBox error={err} />}
      <Async state={st}>
        {(d) => d.runners.length === 0 ? (
          <Blankslate title="There are no runners configured">
            <Link to={`${root}/new`}>Learn more about using runners</Link> to run actions on your own servers.
          </Blankslate>
        ) : (
          <div className="box" data-testid="runners-list">
            <div className="box-header"><strong>{d.total_count} runner{d.total_count === 1 ? "" : "s"}</strong></div>
            <ul className="list">
              {d.runners.map((r) => (
                <RunnerRow key={r.id} scope={scope} runner={r} groupName={gs.data?.find((g) => g.id === r.runner_group_id)?.name} reload={st.reload} onError={setErr} />
              ))}
            </ul>
          </div>
        )}
      </Async>
    </>
  );
}

function RunnerRow({ scope, runner: r, groupName, reload, onError }: { scope: Scope; runner: Runner; groupName?: string; reload: () => void; onError: (e: unknown) => void }) {
  const [label, setLabel] = useState("");
  const [adding, setAdding] = useState(false);
  const status = r.status === "online" ? (r.busy ? "Active" : "Idle") : "Offline";
  const run = async (fn: () => Promise<unknown>) => {
    onError(undefined);
    try {
      await fn();
      reload();
    } catch (e) {
      onError(e);
    }
  };
  return (
    <li className="list-row act-runner-row" data-testid={`runner-${r.name}`}>
      <span className={`act-dot act-dot-${r.status === "online" ? (r.busy ? "busy" : "online") : "offline"}`} title={status} aria-hidden="true" />
      <div className="grow">
        <strong>{r.name}</strong> <span className="muted small">{status}{r.ephemeral ? " · ephemeral" : ""}{groupName ? ` · group ${groupName}` : ""}{r.version ? ` · v${r.version}` : ""}</span>
        <div className="act-labels">
          {r.labels.map((l) => (
            <span key={l.name} className={"badge" + (l.type === "custom" ? " act-label-custom" : "")}>
              {l.name}
              {l.type === "custom" && (
                <button type="button" className="btn-link" aria-label={`Remove label ${l.name}`} onClick={() => run(() => A.removeRunnerLabel(scope, r.id, l.name))}>×</button>
              )}
            </span>
          ))}
          {adding ? (
            <form className="act-inline" onSubmit={(e) => { e.preventDefault(); if (label.trim()) run(() => A.addRunnerLabels(scope, r.id, label.split(",").map((x) => x.trim()).filter(Boolean))).then(() => { setLabel(""); setAdding(false); }); }}>
              <input className="input input-sm" autoFocus value={label} onChange={(e) => setLabel(e.target.value)} placeholder="label, label" aria-label="New labels" data-testid="runner-label-input" />
              <button type="submit" className="btn btn-sm">Add</button>
            </form>
          ) : (
            <button type="button" className="btn-link small" onClick={() => setAdding(true)} data-testid={`runner-add-label-${r.name}`}>+ label</button>
          )}
        </div>
      </div>
      <Dropdown label="…" ariaLabel={`Options for ${r.name}`} testid={`runner-menu-${r.name}`}>
        {(close) => (
          <button role="menuitem" type="button" className="menu-item c-danger" data-testid={`runner-remove-${r.name}`}
            onClick={() => { close(); if (confirm(`Remove runner ${r.name}? A running job is cancelled.`)) run(() => A.deleteRunner(scope, r.id)); }}>Remove runner</button>
        )}
      </Dropdown>
    </li>
  );
}

/** "New self-hosted runner": download, registration token and the `forge runner` command. */
export function NewRunner({ scope, root }: { scope: Scope; root: string }) {
  const tok = useAsync(() => A.registrationToken(scope), [JSON.stringify(scope)]);
  const dl = useAsync(() => A.runnerDownloads(scope).catch(() => []), [JSON.stringify(scope)]);
  const [name, setName] = useState("");
  const [labels, setLabels] = useState("");
  const [executor, setExecutor] = useState<"shell" | "docker">("shell");
  const url = dl.data?.[0]?.download_url || `${origin()}/_actions/runner/forge`;
  return (
    <div className="act-form act-new-runner">
      <p><Link to={root}>Runners</Link> / Create self-hosted runner</p>
      <h2 className="h3">Create self-hosted runner</h2>
      <p className="muted">Adding a self-hosted runner requires that you download, configure, and execute the forge runner. By downloading and configuring the runner, you can run jobs for this {scopeWord(scope)} on your own machine.</p>
      <h3 className="h4">Download</h3>
      <p className="muted small">The runner is part of the <code>forge</code> binary. On the runner machine:</p>
      <CodeLine text={`curl -fLo forge ${url} && chmod +x forge`} label="Download command" />
      <h3 className="h4">Configure</h3>
      <div className="form-row act-runner-opts">
        <div className="form-group"><label htmlFor="rn-name">Runner name (optional)</label><input id="rn-name" className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="defaults to the host name" /></div>
        <div className="form-group"><label htmlFor="rn-labels">Extra labels (optional)</label><input id="rn-labels" className="input" value={labels} onChange={(e) => setLabels(e.target.value)} placeholder="gpu,linux-big" /></div>
        <div className="form-group"><label htmlFor="rn-exec">Executor</label>
          <select id="rn-exec" className="input" value={executor} onChange={(e) => setExecutor(e.target.value as "shell" | "docker")}>
            <option value="shell">shell (the host is the sandbox)</option>
            <option value="docker">docker (job containers)</option>
          </select>
        </div>
      </div>
      <Async state={tok}>
        {(t) => {
          const cmd = ["./forge runner", `--url ${origin()}`, `--token ${t.token}`, name && `--name ${name}`, labels && `--labels ${labels}`, executor !== "shell" && `--executor ${executor}`, "--work-dir ./_work", "--config ./.runner"].filter(Boolean).join(" ");
          return (
            <>
              <div className="form-group">
                <label>Registration token</label>
                <CodeLine text={t.token} label="Registration token" />
                <p className="muted small">This token expires <time dateTime={t.expires_at}>{new Date(t.expires_at).toLocaleString()}</time>. The runner stores its own credential in <code>--config</code> and reuses it on restart.</p>
              </div>
              <div className="form-group">
                <label>Register and start the runner</label>
                <CodeLine text={cmd} label="Runner command" />
              </div>
              <span className="sr-only" data-testid="registration-token">{t.token}</span>
            </>
          );
        }}
      </Async>
      <h3 className="h4">Using your self-hosted runner</h3>
      <p className="muted small">Use this YAML in your workflow file for each job:</p>
      <CodeLine text="runs-on: self-hosted" label="runs-on" />
      <Flash>Jobs with <code>runs-on: ubuntu-latest</code> also run on forge runners: every runner accepts the common hosted labels.</Flash>
    </div>
  );
}

// ---------- runner groups ----------

export function RunnerGroupsList({ scope, root }: { scope: Scope; root: string }) {
  const st = useAsync(() => A.runnerGroups(scope), [JSON.stringify(scope)]);
  return (
    <>
      <div className="act-head">
        <h2 className="h3">Runner groups</h2>
        <Link className="btn btn-sm btn-primary" to={`${root}/new`} data-testid="new-runner-group">New runner group</Link>
      </div>
      <p className="muted">Runner groups control which {scope.kind === "org" ? "repositories" : "organizations"} can use the runners in them.</p>
      <Async state={st}>
        {(d) => (
          <ul className="box list" data-testid="runner-groups">
            {d.runner_groups.map((g) => (
              <li key={g.id} className="list-row">
                <div className="grow">
                  <Link to={`${root}/${g.id}`}><strong>{g.name}</strong></Link> {g.default && <span className="badge">Default</span>} {g.inherited && <span className="badge">Inherited</span>}
                  <div className="muted small">{g.visibility === "all" ? (scope.kind === "org" ? "All repositories" : "All organizations") : g.visibility === "private" ? "Private repositories" : "Selected " + (scope.kind === "org" ? "repositories" : "organizations")}{g.allows_public_repositories ? ", including public repositories" : ""}</div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </>
  );
}

export function RunnerGroupForm({ scope, root, gid }: { scope: Scope; root: string; gid?: string }) {
  const isNew = !gid || gid === "new";
  const id = Number(gid);
  const nav = useNavigate();
  const g = useAsync(() => (isNew ? Promise.resolve(null) : A.runnerGroup(scope, id)), [JSON.stringify(scope), gid]);
  const runners = useAsync(() => A.runners(scope).then((r) => r.runners), [JSON.stringify(scope)]);
  const members = useAsync(() => (isNew ? Promise.resolve([] as Runner[]) : A.runnerGroupRunners(scope, id).then((r) => r.runners)), [JSON.stringify(scope), gid]);
  const targets = useAsync(
    () => scope.kind === "org"
      ? A.orgRepos(scope.org).then((rs) => rs.map((r) => ({ id: r.id, name: r.full_name })))
      : A.allOrgs().then((os) => os.map((o) => ({ id: o.id, name: o.login }))),
    [JSON.stringify(scope)],
  );
  const selected = useAsync(
    () => isNew ? Promise.resolve([] as number[]) : scope.kind === "org"
      ? A.runnerGroupRepos(scope, id).then((r) => r.repositories.map((x) => x.id)).catch(() => [])
      : A.runnerGroupOrgs(scope, id).then((r) => r.organizations.map((x) => x.id)).catch(() => []),
    [JSON.stringify(scope), gid],
  );
  if (g.data === undefined || members.data === undefined || selected.data === undefined) return <Async state={g}>{() => null}</Async>;
  return <GroupEditor key={gid} scope={scope} root={root} group={g.data} runners={runners.data || []} members={members.data.map((r) => r.id)}
    targets={targets.data || []} selected={selected.data} onDone={() => nav(root)} />;
}

function GroupEditor({ scope, root, group, runners, members, targets, selected, onDone }: {
  scope: Scope; root: string; group: RunnerGroup | null; runners: Runner[]; members: number[]; targets: { id: number; name: string }[]; selected: number[]; onDone: () => void;
}) {
  const [name, setName] = useState(group?.name || "");
  const [vis, setVis] = useState(group?.visibility || "all");
  const [pub, setPub] = useState(group?.allows_public_repositories ?? false);
  const [restrict, setRestrict] = useState(group?.restricted_to_workflows ?? false);
  const [wfs, setWfs] = useState((group?.selected_workflows || []).join("\n"));
  const [mem, setMem] = useState<number[]>(members);
  const [sel, setSel] = useState<number[]>(selected);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>();
  const what = scope.kind === "org" ? "repositories" : "organizations";
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(undefined);
    const body: Record<string, unknown> = {
      name, visibility: vis, allows_public_repositories: pub,
      restricted_to_workflows: restrict, selected_workflows: restrict ? wfs.split("\n").map((x) => x.trim()).filter(Boolean) : [],
    };
    try {
      if (!group) {
        body.runners = mem;
        if (vis === "selected") body[scope.kind === "org" ? "selected_repository_ids" : "selected_organization_ids"] = sel;
        await A.createRunnerGroup(scope, body);
      } else {
        await A.updateRunnerGroup(scope, group.id, group.default ? { ...body, name: undefined, visibility: vis } : body);
        await A.setRunnerGroupRunners(scope, group.id, mem);
        if (vis === "selected") {
          if (scope.kind === "org") await A.setRunnerGroupRepos(scope, group.id, sel);
          else await A.setRunnerGroupOrgs(scope, group.id, sel);
        }
      }
      onDone();
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  const del = async () => {
    if (!group || !confirm(`Remove runner group ${group.name}? Its runners move to the default group.`)) return;
    try {
      await A.deleteRunnerGroup(scope, group.id);
      onDone();
    } catch (e) {
      setErr(e);
    }
  };
  return (
    <form className="act-form" onSubmit={submit} data-testid="runner-group-form">
      <p><Link to={root}>Runner groups</Link> / {group ? group.name : "New group"}</p>
      <h2 className="h3">{group ? group.name : "Create runner group"}</h2>
      <div className="form-group">
        <label htmlFor="rg-name">Group name</label>
        <input id="rg-name" className="input" value={name} onChange={(e) => setName(e.target.value)} disabled={!!group?.default} required />
      </div>
      <div className="form-group">
        <label htmlFor="rg-vis">{scope.kind === "org" ? "Repository access" : "Organization access"}</label>
        <select id="rg-vis" className="input input-auto" value={vis} onChange={(e) => setVis(e.target.value)}>
          <option value="all">All {what}</option>
          <option value="selected">Selected {what}</option>
          {scope.kind === "org" && <option value="private">Private repositories</option>}
        </select>
        <label className="check-row"><input type="checkbox" checked={pub} onChange={(e) => setPub(e.target.checked)} /> <span>Allow public repositories</span></label>
      </div>
      {vis === "selected" && (
        <div className="box act-repo-pick">
          <div className="box-header"><strong>{sel.length} selected {what}</strong></div>
          {targets.map((t) => (
            <label key={t.id} className="check-row act-pick-row">
              <input type="checkbox" checked={sel.includes(t.id)} onChange={(e) => setSel(e.target.checked ? [...sel, t.id] : sel.filter((x) => x !== t.id))} /> <span>{t.name}</span>
            </label>
          ))}
          {!targets.length && <div className="box-body muted small">Nothing to select.</div>}
        </div>
      )}
      <div className="form-group">
        <label className="check-row"><input type="checkbox" checked={restrict} onChange={(e) => setRestrict(e.target.checked)} /> <span>Restrict to selected workflows</span></label>
        {restrict && <textarea className="input textarea mono" rows={3} value={wfs} onChange={(e) => setWfs(e.target.value)} placeholder="octo-org/repo/.github/workflows/deploy.yml@main" aria-label="Selected workflows" />}
      </div>
      <div className="box act-repo-pick">
        <div className="box-header"><strong>Runners</strong> <span className="muted small">{mem.length} in this group</span></div>
        {runners.map((r) => (
          <label key={r.id} className="check-row act-pick-row">
            <input type="checkbox" checked={mem.includes(r.id)} onChange={(e) => setMem(e.target.checked ? [...mem, r.id] : mem.filter((x) => x !== r.id))} /> <span>{r.name}</span>
          </label>
        ))}
        {!runners.length && <div className="box-body muted small">No runners registered at this level.</div>}
      </div>
      {err !== undefined && <ErrorBox error={err} />}
      <div className="form-actions act-left">
        <button type="submit" className="btn btn-primary" disabled={busy} data-testid="runner-group-save">{group ? "Save group" : "Create group"}</button>
        {group && !group.default && <button type="button" className="btn btn-danger-outline" onClick={del} data-testid="runner-group-delete">Remove group</button>}
      </div>
    </form>
  );
}

// Settings → Branches: default branch, classic branch protection rules (list, full editor, delete).
//   /{o}/{r}/settings/branches               list
//   /{o}/{r}/settings/branches/new           new rule
//   /{o}/{r}/settings/branches/edit/<branch> edit a rule
import { useEffect, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { Async, Blankslate, Spinner } from "../../components/common";
import type { RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { ra, type Protection, type ProtectionInput } from "./api";
import { Check, ListInput, Pill, SectionHead, Status, useAction } from "./ui";

export function BranchesSettings({ ctx }: { ctx: RepoCtx }) {
  const { "*": splat = "" } = useParams();
  const rest = splat.replace(/^branches\/?/, "");
  if (rest === "new") return <ProtectionEditor ctx={ctx} />;
  if (rest.startsWith("edit/")) return <ProtectionEditor ctx={ctx} branch={decodeURIComponent(rest.slice(5))} />;
  return <BranchesList ctx={ctx} />;
}

function BranchesList({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Branches · ${ctx.repo.full_name}`);
  const st = useAsync(() => ra.protectedBranches(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const act = useAction();
  const base = `${ctx.base}/settings/branches`;
  return (
    <div className="settings ra-branches">
      <SectionHead title="Branches" />
      <section className="box">
        <div className="box-header"><h3 className="h5">Default branch</h3></div>
        <div className="box-body ra-default-branch">
          <p className="muted small">The default branch is considered the “base” branch in your repository, against which all pull requests and code commits are automatically made, unless you specify a different branch.</p>
          <div className="ra-row-inline">
            <span className="branch-name mono" data-testid="default-branch">{ctx.repo.default_branch}</span>
            <Link className="btn btn-sm" to={`${ctx.base}/settings`}>Switch to another branch</Link>
          </div>
        </div>
      </section>

      <section className="box">
        <div className="box-header ra-box-header">
          <h3 className="h5">Branch protection rules</h3>
          <div className="ra-head-actions">
            <Link className="btn btn-sm" to={`${ctx.base}/settings/rules/new?target=branch`}>Add branch ruleset</Link>
            <Link className="btn btn-sm btn-primary" to={`${base}/new`}>Add classic branch protection rule</Link>
          </div>
        </div>
        <p className="box-body muted small ra-nomargin">
          Define branch protection rules to disable force pushing, prevent branches from being deleted, and optionally require status checks before merging.
        </p>
        <Status state={act.state} />
        <Async state={st}>
          {(list) =>
            list.length === 0 ? (
              <Blankslate title="No branch protection rules defined yet." />
            ) : (
              <ul className="ra-list" data-testid="protection-rules">
                {list.map((b) => (
                  <li key={b.name} className="ra-row" data-testid={`protection-${b.name}`}>
                    <div className="ra-row-main">
                      <span className="branch-name mono">{b.name}</span>
                      <ProtectionSummary ctx={ctx} branch={b.name} />
                    </div>
                    <div className="ra-row-actions">
                      <Link className="btn btn-sm" to={`${base}/edit/${encodeURIComponent(b.name)}`}>Edit</Link>
                      <button type="button" className="btn btn-sm btn-danger" disabled={act.busy}
                        onClick={async () => {
                          if (!confirm(`Delete the branch protection rule for ${b.name}?`)) return;
                          await act.run(() => ra.deleteProtection(ctx.owner, ctx.name, b.name), `Branch protection rule for ${b.name} deleted.`);
                          st.reload();
                        }}>Delete</button>
                    </div>
                  </li>
                ))}
              </ul>
            )
          }
        </Async>
      </section>
    </div>
  );
}

function ProtectionSummary({ ctx, branch }: { ctx: RepoCtx; branch: string }) {
  const st = useAsync(() => ra.protection(ctx.owner, ctx.name, branch), [ctx.owner, ctx.name, branch]);
  const p = st.data;
  if (!p) return null;
  const bits: string[] = [];
  if (p.required_pull_request_reviews) bits.push(`${p.required_pull_request_reviews.required_approving_review_count ?? 0} approvals`);
  if (p.required_status_checks) bits.push(`${p.required_status_checks.contexts.length} status checks`);
  if (p.required_signatures?.enabled) bits.push("signed commits");
  if (p.required_linear_history?.enabled) bits.push("linear history");
  if (p.enforce_admins?.enabled) bits.push("includes administrators");
  if (p.lock_branch?.enabled) bits.push("locked");
  if (p.allow_force_pushes?.enabled) bits.push("force pushes allowed");
  return <div className="small muted">{bits.length ? bits.join(" · ") : "Protected"}</div>;
}

interface Form {
  branch: string;
  requirePr: boolean;
  approvals: number;
  dismissStale: boolean;
  codeOwners: boolean;
  lastPush: boolean;
  restrictDismissals: boolean;
  dismissUsers: string[];
  dismissTeams: string[];
  allowBypass: boolean;
  bypassUsers: string[];
  bypassTeams: string[];
  requireChecks: boolean;
  strict: boolean;
  contexts: string[];
  conversation: boolean;
  signatures: boolean;
  linear: boolean;
  lock: boolean;
  forkSync: boolean;
  enforceAdmins: boolean;
  restrictPush: boolean;
  pushUsers: string[];
  pushTeams: string[];
  pushApps: string[];
  blockCreations: boolean;
  forcePush: boolean;
  deletions: boolean;
}

const empty: Form = {
  branch: "", requirePr: false, approvals: 1, dismissStale: false, codeOwners: false, lastPush: false,
  restrictDismissals: false, dismissUsers: [], dismissTeams: [], allowBypass: false, bypassUsers: [], bypassTeams: [],
  requireChecks: false, strict: false, contexts: [], conversation: false, signatures: false, linear: false, lock: false,
  forkSync: false, enforceAdmins: false, restrictPush: false, pushUsers: [], pushTeams: [], pushApps: [], blockCreations: false,
  forcePush: false, deletions: false,
};

function fromProtection(branch: string, p: Protection): Form {
  const pr = p.required_pull_request_reviews;
  const rs = p.restrictions;
  return {
    branch,
    requirePr: !!pr,
    approvals: pr?.required_approving_review_count ?? 1,
    dismissStale: !!pr?.dismiss_stale_reviews,
    codeOwners: !!pr?.require_code_owner_reviews,
    lastPush: !!pr?.require_last_push_approval,
    restrictDismissals: !!pr?.dismissal_restrictions && ((pr.dismissal_restrictions.users?.length || 0) + (pr.dismissal_restrictions.teams?.length || 0) > 0),
    dismissUsers: (pr?.dismissal_restrictions?.users || []).map((u) => u.login),
    dismissTeams: (pr?.dismissal_restrictions?.teams || []).map((t) => t.slug),
    allowBypass: !!pr?.bypass_pull_request_allowances && ((pr.bypass_pull_request_allowances.users?.length || 0) + (pr.bypass_pull_request_allowances.teams?.length || 0) > 0),
    bypassUsers: (pr?.bypass_pull_request_allowances?.users || []).map((u) => u.login),
    bypassTeams: (pr?.bypass_pull_request_allowances?.teams || []).map((t) => t.slug),
    requireChecks: !!p.required_status_checks,
    strict: !!p.required_status_checks?.strict,
    contexts: p.required_status_checks?.contexts || [],
    conversation: !!p.required_conversation_resolution?.enabled,
    signatures: !!p.required_signatures?.enabled,
    linear: !!p.required_linear_history?.enabled,
    lock: !!p.lock_branch?.enabled,
    forkSync: !!p.allow_fork_syncing?.enabled,
    enforceAdmins: !!p.enforce_admins?.enabled,
    restrictPush: !!rs,
    pushUsers: (rs?.users || []).map((u) => u.login),
    pushTeams: (rs?.teams || []).map((t) => t.slug),
    pushApps: (rs?.apps || []).map((a) => a.slug),
    blockCreations: !!p.block_creations?.enabled,
    forcePush: !!p.allow_force_pushes?.enabled,
    deletions: !!p.allow_deletions?.enabled,
  };
}

function toInput(f: Form, isOrg: boolean): ProtectionInput {
  return {
    required_status_checks: f.requireChecks ? { strict: f.strict, contexts: f.contexts } : null,
    enforce_admins: f.enforceAdmins,
    required_pull_request_reviews: f.requirePr
      ? {
          required_approving_review_count: f.approvals,
          dismiss_stale_reviews: f.dismissStale,
          require_code_owner_reviews: f.codeOwners,
          require_last_push_approval: f.lastPush,
          ...(isOrg ? { dismissal_restrictions: f.restrictDismissals ? { users: f.dismissUsers, teams: f.dismissTeams } : { users: [], teams: [] } } : {}),
          bypass_pull_request_allowances: f.allowBypass ? { users: f.bypassUsers, teams: f.bypassTeams } : { users: [], teams: [] },
        }
      : null,
    restrictions: isOrg && f.restrictPush ? { users: f.pushUsers, teams: f.pushTeams, apps: f.pushApps } : null,
    required_linear_history: f.linear,
    allow_force_pushes: f.forcePush,
    allow_deletions: f.deletions,
    block_creations: isOrg && f.restrictPush ? f.blockCreations : false,
    required_conversation_resolution: f.conversation,
    lock_branch: f.lock,
    allow_fork_syncing: f.forkSync,
  };
}

function ProtectionEditor({ ctx, branch }: { ctx: RepoCtx; branch?: string }) {
  useTitle(`${branch ? "Edit" : "New"} branch protection rule · ${ctx.repo.full_name}`);
  const nav = useNavigate();
  const isOrg = ctx.repo.owner.type === "Organization";
  const branches = useAsync(() => ra.branches(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const existing = useAsync(() => (branch ? ra.protection(ctx.owner, ctx.name, branch) : Promise.resolve(null)), [ctx.owner, ctx.name, branch]);
  const [f, setF] = useState<Form>({ ...empty, branch: branch || "" });
  const act = useAction();
  useEffect(() => {
    if (branch && existing.data) setF(fromProtection(branch, existing.data));
  }, [branch, existing.data]);
  const set = <K extends keyof Form>(k: K) => (v: Form[K]) => setF((x) => ({ ...x, [k]: v }));
  if (branch && existing.loading) return <Spinner />;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const ok = await act.run(async () => {
      await ra.putProtection(ctx.owner, ctx.name, f.branch, toInput(f, isOrg));
      await ra.setSignatures(ctx.owner, ctx.name, f.branch, f.signatures);
      return true;
    });
    if (ok) nav(`${ctx.base}/settings/branches`);
  };
  const taken = new Set((branches.data || []).filter((b) => b.protected).map((b) => b.name));
  return (
    <form className="settings ra-protection" onSubmit={submit} data-testid="protection-editor">
      <div className="ra-crumbs small"><Link to={`${ctx.base}/settings/branches`}>Branch protection rules</Link> / {branch ? "Edit" : "Add rule"}</div>
      <SectionHead title={branch ? "Edit branch protection rule" : "Branch protection rule"} />
      <Status state={act.state} />
      <div className="box box-body">
        <div className="form-group">
          <label htmlFor="bp-branch">Branch name</label>
          {branch ? (
            <input id="bp-branch" className="input mono" value={branch} readOnly />
          ) : (
            <select id="bp-branch" className="input" required value={f.branch} onChange={(e) => set("branch")(e.target.value)}>
              <option value="">Select a branch</option>
              {(branches.data || []).map((b) => (
                <option key={b.name} value={b.name} disabled={taken.has(b.name)}>{b.name}{taken.has(b.name) ? " (already protected)" : ""}</option>
              ))}
            </select>
          )}
          <p className="small muted">Classic rules protect one branch; use a <Link to={`${ctx.base}/settings/rules/new?target=branch`}>ruleset</Link> to target patterns like <code>release/*</code>.</p>
        </div>
      </div>

      <h3 className="h4">Protect matching branches</h3>
      <div className="box box-body">
        <Check id="bp-pr" label="Require a pull request before merging" checked={f.requirePr} onChange={set("requirePr")}
          desc="When enabled, all commits must be made to a non-protected branch and submitted via a pull request before they can be merged into this branch.">
          <div className="form-group">
            <label htmlFor="bp-approvals">Required number of approvals before merging</label>
            <select id="bp-approvals" className="input input-auto" value={f.approvals} onChange={(e) => set("approvals")(Number(e.target.value))}>
              {[0, 1, 2, 3, 4, 5, 6].map((n) => <option key={n} value={n}>{n}</option>)}
            </select>
          </div>
          <Check id="bp-stale" label="Dismiss stale pull request approvals when new commits are pushed" checked={f.dismissStale} onChange={set("dismissStale")}
            desc="New reviewable commits pushed to a matching branch will dismiss pull request review approvals." />
          <Check id="bp-codeowners" label="Require review from Code Owners" checked={f.codeOwners} onChange={set("codeOwners")}
            desc="Require an approved review in pull requests including files with a designated code owner." />
          <Check id="bp-lastpush" label="Require approval of the most recent reviewable push" checked={f.lastPush} onChange={set("lastPush")}
            desc="Whether the most recent reviewable push must be approved by someone other than the person who pushed it." />
          {isOrg && (
            <Check id="bp-dismissers" label="Restrict who can dismiss pull request reviews" checked={f.restrictDismissals} onChange={set("restrictDismissals")}>
              <ActorLists idp="bp-dismiss" users={f.dismissUsers} teams={f.dismissTeams} setUsers={set("dismissUsers")} setTeams={set("dismissTeams")} />
            </Check>
          )}
          <Check id="bp-bypass" label="Allow specified actors to bypass required pull requests" checked={f.allowBypass} onChange={set("allowBypass")}>
            <ActorLists idp="bp-bypass" users={f.bypassUsers} teams={f.bypassTeams} setUsers={set("bypassUsers")} setTeams={set("bypassTeams")} showTeams={isOrg} />
          </Check>
        </Check>
        <Check id="bp-checks" label="Require status checks to pass before merging" checked={f.requireChecks} onChange={set("requireChecks")}
          desc="Choose which status checks must pass before branches can be merged into a branch that matches this rule.">
          <Check id="bp-strict" label="Require branches to be up to date before merging" checked={f.strict} onChange={set("strict")}
            desc="This ensures pull requests targeting a matching branch have been tested with the latest code." />
          <div className="form-group">
            <label htmlFor="bp-contexts">Status checks that are required (one per line)</label>
            <ListInput id="bp-contexts" value={f.contexts} onChange={set("contexts")} placeholder="ci/build" />
          </div>
        </Check>
        <Check id="bp-conversation" label="Require conversation resolution before merging" checked={f.conversation} onChange={set("conversation")}
          desc="When enabled, all conversations on code must be resolved before a pull request can be merged into a branch that matches this rule." />
        <Check id="bp-signatures" label="Require signed commits" checked={f.signatures} onChange={set("signatures")}
          desc="Commits pushed to matching branches must have verified signatures." />
        <Check id="bp-linear" label="Require linear history" checked={f.linear} onChange={set("linear")}
          desc="Prevent merge commits from being pushed to matching branches." />
        <Check id="bp-lock" label="Lock branch" checked={f.lock} onChange={set("lock")}
          desc="Branch is read-only. Users cannot push to the branch." />
        {f.lock && (
          <div className="ra-check-children">
            <Check id="bp-forksync" label="Allow fork syncing" checked={f.forkSync} onChange={set("forkSync")}
              desc="Branch is read-only, but users can pull changes from the upstream repository." />
          </div>
        )}
        <Check id="bp-admins" label="Do not allow bypassing the above settings" checked={f.enforceAdmins} onChange={set("enforceAdmins")}
          desc="The above settings will apply to administrators and custom roles with the “bypass branch protections” permission." />
        {isOrg && (
          <Check id="bp-restrict" label="Restrict who can push to matching branches" checked={f.restrictPush} onChange={set("restrictPush")}
            desc="Specify people, teams, or apps allowed to push to matching branches. Required status checks will still prevent these people, teams, and apps from merging if the checks fail.">
            <Check id="bp-block-create" label="Restrict pushes that create matching branches" checked={f.blockCreations} onChange={set("blockCreations")}
              desc="Only people, teams, or apps allowed to push will be able to create new branches matching this rule." />
            <ActorLists idp="bp-push" users={f.pushUsers} teams={f.pushTeams} setUsers={set("pushUsers")} setTeams={set("pushTeams")} />
            <div className="form-group">
              <label htmlFor="bp-push-apps">Apps (slugs)</label>
              <ListInput id="bp-push-apps" value={f.pushApps} onChange={set("pushApps")} rows={2} />
            </div>
          </Check>
        )}
      </div>

      <h3 className="h4">Rules applied to everyone including administrators</h3>
      <div className="box box-body">
        <Check id="bp-force" label="Allow force pushes" checked={f.forcePush} onChange={set("forcePush")}
          desc="Permit force pushes for all users with push access." />
        <Check id="bp-deletions" label="Allow deletions" checked={f.deletions} onChange={set("deletions")}
          desc="Allow users with push access to delete matching branches." />
      </div>
      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={act.busy || !f.branch}>{branch ? "Save changes" : "Create"}</button>
        <Link className="btn" to={`${ctx.base}/settings/branches`}>Cancel</Link>
        {branch && <Pill kind="info">Rulesets also apply: see <Link to={`${ctx.base}/settings/rules`}>Rulesets</Link></Pill>}
      </div>
    </form>
  );
}

function ActorLists({ idp, users, teams, setUsers, setTeams, showTeams = true }: {
  idp: string; users: string[]; teams: string[]; setUsers: (v: string[]) => void; setTeams: (v: string[]) => void; showTeams?: boolean;
}) {
  return (
    <div className="form-row ra-actor-lists">
      <div className="form-group">
        <label htmlFor={`${idp}-users`}>Users (logins, one per line)</label>
        <ListInput id={`${idp}-users`} value={users} onChange={setUsers} rows={2} />
      </div>
      {showTeams && (
        <div className="form-group">
          <label htmlFor={`${idp}-teams`}>Teams (slugs)</label>
          <ListInput id={`${idp}-teams`} value={teams} onChange={setTeams} rows={2} />
        </div>
      )}
    </div>
  );
}

// Settings → General extras (rendered through the "repo.settings.general" / "repo.settings.danger"
// slots of the core General page): template repository, features, pull request merge options,
// commits, archives/LFS, and the Danger Zone actions (visibility, transfer, archive).
import { useEffect, useState, type FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { request } from "../../api";
import type { RepoCtx } from "../../components/RepoLayout";
import { useAsync } from "../../hooks";
import { ra, type RepoFull } from "./api";
import { Check, DangerRow, Status, useAction } from "./ui";

const lfsUrl = (o: string, r: string) => `/api/forge/repos/${encodeURIComponent(o)}/${encodeURIComponent(r)}/lfs`;

export function GeneralExtras({ ctx }: { ctx: RepoCtx }) {
  const repo = ctx.repo as RepoFull;
  const act = useAction();
  const [r, setR] = useState<RepoFull>(repo);
  useEffect(() => setR(ctx.repo as RepoFull), [ctx.repo]);
  const lfs = useAsync(() => request<{ enabled: boolean }>(lfsUrl(ctx.owner, ctx.name)).catch(() => ({ enabled: false })), [ctx.owner, ctx.name]);

  /** Autosave one or more fields (GitHub saves these checkboxes immediately). */
  const save = async (patch: Partial<RepoFull>) => {
    const prev = r;
    setR({ ...r, ...patch });
    const out = await act.run(() => ra.updateRepo(ctx.owner, ctx.name, patch), "Saved.");
    if (out) ctx.setRepo(out);
    else setR(prev);
  };
  const isOrg = repo.owner.type === "Organization";
  const noMerge = !r.allow_merge_commit && !r.allow_squash_merge && !r.allow_rebase_merge;
  const mergeOff = (k: "allow_merge_commit" | "allow_squash_merge" | "allow_rebase_merge") =>
    r[k] && [r.allow_merge_commit, r.allow_squash_merge, r.allow_rebase_merge].filter(Boolean).length === 1;

  return (
    <div className="ra-general" data-testid="general-extras">
      <Status state={act.state} />
      <section className="box box-body">
        <Check id="gen-template" label="Template repository" checked={!!r.is_template} disabled={act.busy}
          desc="Template repositories let users generate new repositories with the same directory structure and files."
          onChange={(v) => save({ is_template: v })} />
      </section>

      <h2 className="h3">Features</h2>
      <section className="box box-body" aria-label="Features">
        <Check id="gen-wiki" label="Wikis" checked={!!r.has_wiki} disabled={act.busy}
          desc="Wikis host documentation for your repository." onChange={(v) => save({ has_wiki: v })} />
        <Check id="gen-issues" label="Issues" checked={!!r.has_issues} disabled={act.busy}
          desc="Issues integrate lightweight task tracking into your repository." onChange={(v) => save({ has_issues: v })} />
        <Check id="gen-discussions" label="Discussions" checked={!!r.has_discussions} disabled={act.busy}
          desc="Discussions is the space for your community to have conversations, ask questions and post answers without opening issues."
          onChange={(v) => save({ has_discussions: v })} />
        <Check id="gen-projects" label="Projects" checked={!!r.has_projects} disabled={act.busy}
          desc="Projects on GitHub are created at the repository owner's level and can be linked to this repository."
          onChange={(v) => save({ has_projects: v })} />
        {isOrg && r.private && (
          <Check id="gen-forking" label="Allow forking" checked={r.allow_forking !== false} disabled={act.busy}
            desc="If enabled, forking is allowed for this private repository." onChange={(v) => save({ allow_forking: v })} />
        )}
      </section>

      <h2 className="h3">Pull Requests</h2>
      <section className="box box-body" aria-label="Pull Requests">
        <p className="muted small">When merging pull requests, you can allow any combination of merge commits, squashing, or rebasing. At least one option must be enabled.</p>
        <Check id="gen-merge" label="Allow merge commits" checked={!!r.allow_merge_commit} disabled={act.busy || mergeOff("allow_merge_commit")}
          desc="Add all commits from the head branch to the base branch with a merge commit." onChange={(v) => save({ allow_merge_commit: v })}>
          <label className="ra-inline-label" htmlFor="gen-merge-msg">Default commit message</label>
          <select id="gen-merge-msg" className="input input-auto" value={`${r.merge_commit_title}|${r.merge_commit_message}`} disabled={act.busy}
            onChange={(e) => {
              const [t, m] = e.target.value.split("|");
              save({ merge_commit_title: t, merge_commit_message: m });
            }}>
            <option value="MERGE_MESSAGE|PR_TITLE">Default message</option>
            <option value="PR_TITLE|PR_BODY">Pull request title and description</option>
            <option value="PR_TITLE|BLANK">Pull request title</option>
          </select>
        </Check>
        <Check id="gen-squash" label="Allow squash merging" checked={!!r.allow_squash_merge} disabled={act.busy || mergeOff("allow_squash_merge")}
          desc="Combine all commits from the head branch into a single commit in the base branch." onChange={(v) => save({ allow_squash_merge: v })}>
          <label className="ra-inline-label" htmlFor="gen-squash-msg">Default commit message</label>
          <select id="gen-squash-msg" className="input input-auto" value={`${r.squash_merge_commit_title}|${r.squash_merge_commit_message}`} disabled={act.busy}
            onChange={(e) => {
              const [t, m] = e.target.value.split("|");
              save({ squash_merge_commit_title: t, squash_merge_commit_message: m });
            }}>
            <option value="COMMIT_OR_PR_TITLE|COMMIT_MESSAGES">Default message</option>
            <option value="PR_TITLE|PR_BODY">Pull request title and description</option>
            <option value="PR_TITLE|COMMIT_MESSAGES">Pull request title and commit details</option>
            <option value="PR_TITLE|BLANK">Pull request title</option>
          </select>
        </Check>
        <Check id="gen-rebase" label="Allow rebase merging" checked={!!r.allow_rebase_merge} disabled={act.busy || mergeOff("allow_rebase_merge")}
          desc="Add all commits from the head branch onto the base branch individually." onChange={(v) => save({ allow_rebase_merge: v })} />
        {noMerge && <p className="flash flash-warn">You must select at least one option.</p>}
        <hr className="ra-hr" />
        <Check id="gen-update-branch" label="Always suggest updating pull request branches" checked={!!r.allow_update_branch} disabled={act.busy}
          desc="Whenever there are new changes available in the base branch, present an “update branch” option in the pull request."
          onChange={(v) => save({ allow_update_branch: v })} />
        <Check id="gen-auto-merge" label="Allow auto-merge" checked={!!r.allow_auto_merge} disabled={act.busy}
          desc="Waits for merge requirements to be met and then merges automatically." onChange={(v) => save({ allow_auto_merge: v })} />
        <Check id="gen-delete-head" label="Automatically delete head branches" checked={!!r.delete_branch_on_merge} disabled={act.busy}
          desc="Deleted branches will still be able to be restored." onChange={(v) => save({ delete_branch_on_merge: v })} />
      </section>

      <h2 className="h3">Commits</h2>
      <section className="box box-body" aria-label="Commits">
        <Check id="gen-signoff" label="Require contributors to sign off on web-based commits" checked={!!r.web_commit_signoff_required} disabled={act.busy}
          desc="Enabling this setting will require contributors to sign off on commits made through GitHub's web interface."
          onChange={(v) => save({ web_commit_signoff_required: v })} />
      </section>

      <h2 className="h3">Archives</h2>
      <section className="box box-body" aria-label="Archives">
        <Check id="gen-lfs" label="Git Large File Storage (LFS)" checked={!!lfs.data?.enabled} disabled={act.busy || lfs.loading}
          desc="Enable Git LFS for this repository (LFS objects are then also included in source code archives)."
          onChange={async (v) => {
            const ok = await act.run(() => (v ? ra.enableLfs(ctx.owner, ctx.name) : ra.disableLfs(ctx.owner, ctx.name)), v ? "Git LFS enabled." : "Git LFS disabled.");
            if (ok !== undefined || v === false) lfs.reload();
            else lfs.reload();
          }} />
      </section>
    </div>
  );
}

export function DangerExtras({ ctx }: { ctx: RepoCtx }) {
  const repo = ctx.repo as RepoFull;
  const nav = useNavigate();
  return (
    <div className="box danger-zone ra-danger" data-testid="danger-extras">
      <DangerRow testid="danger-visibility" title="Change repository visibility"
        desc={`This repository is currently ${repo.visibility || (repo.private ? "private" : "public")}.`} button="Change visibility">
        {(close) => <VisibilityForm ctx={ctx} close={close} />}
      </DangerRow>
      <DangerRow testid="danger-transfer" title="Transfer ownership"
        desc="Transfer this repository to another user or to an organization where you have the ability to create repositories." button="Transfer">
        {(close) => <TransferForm ctx={ctx} close={close} onDone={(full) => nav(`/${full}`)} />}
      </DangerRow>
      <DangerRow testid="danger-archive" title={repo.archived ? "Unarchive this repository" : "Archive this repository"}
        desc={repo.archived ? "Mark this repository as unarchived and read-write." : "Mark this repository as archived and read-only."}
        button={repo.archived ? "Unarchive this repository" : "Archive this repository"}>
        {(close) => <ArchiveForm ctx={ctx} close={close} />}
      </DangerRow>
    </div>
  );
}

function ConfirmName({ full, value, onChange, id }: { full: string; value: string; onChange: (v: string) => void; id: string }) {
  return (
    <div className="form-group">
      <label htmlFor={id}>To confirm, type <strong>{full}</strong> in the box below</label>
      <input id={id} className="input" value={value} autoComplete="off" onChange={(e) => onChange(e.target.value)} />
    </div>
  );
}

function VisibilityForm({ ctx, close }: { ctx: RepoCtx; close: () => void }) {
  const repo = ctx.repo as RepoFull;
  const cur = repo.visibility || (repo.private ? "private" : "public");
  const options = repo.owner.type === "Organization" ? ["public", "internal", "private"] : ["public", "private"];
  const [vis, setVis] = useState(options.find((o) => o !== cur) || "private");
  const [confirm, setConfirm] = useState("");
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const r = await act.run(() => ra.updateRepo(ctx.owner, ctx.name, { visibility: vis }));
    if (r) {
      ctx.setRepo(r);
      close();
    }
  };
  return (
    <form onSubmit={submit}>
      <Status state={act.state} />
      <fieldset className="form-group">
        <legend>New visibility</legend>
        {options.map((o) => (
          <label key={o} className="radio-row">
            <input type="radio" name="vis" value={o} checked={vis === o} disabled={o === cur} onChange={() => setVis(o)} />
            <span>Change to {o}{o === cur ? " (current)" : ""}</span>
          </label>
        ))}
      </fieldset>
      <ConfirmName id="vis-confirm" full={repo.full_name} value={confirm} onChange={setConfirm} />
      <div className="form-actions">
        <button type="submit" className="btn btn-danger" disabled={act.busy || confirm !== repo.full_name}>I understand, change repository visibility.</button>
        <button type="button" className="btn" onClick={close}>Cancel</button>
      </div>
    </form>
  );
}

function TransferForm({ ctx, close, onDone }: { ctx: RepoCtx; close: () => void; onDone: (full: string) => void }) {
  const repo = ctx.repo as RepoFull;
  const orgs = useAsync(() => ra.myOrgs().catch(() => []), []);
  const [owner, setOwner] = useState("");
  const [newName, setNewName] = useState(repo.name);
  const [confirm, setConfirm] = useState("");
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const r = await act.run(() => ra.transfer(ctx.owner, ctx.name, { new_owner: owner.trim(), new_name: newName !== repo.name ? newName : undefined }));
    if (r) onDone(r.full_name || `${owner.trim()}/${newName}`);
  };
  return (
    <form onSubmit={submit}>
      <Status state={act.state} />
      <div className="form-group">
        <label htmlFor="transfer-owner">New owner</label>
        <input id="transfer-owner" className="input" required value={owner} list="transfer-owner-list" autoComplete="off"
          placeholder="Username or organization" onChange={(e) => setOwner(e.target.value)} />
        <datalist id="transfer-owner-list">
          {(orgs.data || []).map((o) => <option key={o.login} value={o.login} />)}
        </datalist>
      </div>
      <div className="form-group">
        <label htmlFor="transfer-name">Repository name</label>
        <input id="transfer-name" className="input" required value={newName} onChange={(e) => setNewName(e.target.value)} />
      </div>
      <ConfirmName id="transfer-confirm" full={repo.full_name} value={confirm} onChange={setConfirm} />
      <div className="form-actions">
        <button type="submit" className="btn btn-danger" disabled={act.busy || !owner.trim() || confirm !== repo.full_name}>I understand, transfer this repository.</button>
        <button type="button" className="btn" onClick={close}>Cancel</button>
      </div>
    </form>
  );
}

function ArchiveForm({ ctx, close }: { ctx: RepoCtx; close: () => void }) {
  const repo = ctx.repo as RepoFull;
  const [confirm, setConfirm] = useState("");
  const act = useAction();
  const archiving = !repo.archived;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const r = await act.run(() => ra.updateRepo(ctx.owner, ctx.name, { archived: archiving }));
    if (r) {
      ctx.setRepo(r);
      close();
    }
  };
  return (
    <form onSubmit={submit}>
      <Status state={act.state} />
      <p className="small">
        {archiving
          ? "This repository will become read-only: issues, pull requests, labels, milestones, wikis and pushes are disabled until it is unarchived."
          : "This repository will become writable again."}
      </p>
      <ConfirmName id="archive-confirm" full={repo.full_name} value={confirm} onChange={setConfirm} />
      <div className="form-actions">
        <button type="submit" className="btn btn-danger" disabled={act.busy || confirm !== repo.full_name}>
          {archiving ? "I understand the consequences, archive this repository" : "I understand the consequences, unarchive this repository"}
        </button>
        <button type="button" className="btn" onClick={close}>Cancel</button>
      </div>
    </form>
  );
}

/** Banner on the Code page area: shown at the top of the General settings for archived repos. */
export function ArchivedNote({ ctx }: { ctx: RepoCtx }) {
  const repo = ctx.repo as RepoFull;
  if (!repo.archived) return null;
  return <div className="flash flash-warn" data-testid="archived-banner">This repository has been archived by the owner. It is now read-only.</div>;
}

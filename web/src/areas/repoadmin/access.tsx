// Settings → Collaborators and teams (`/{o}/{r}/settings/access`), plus the invitee's side:
// `/{o}/{r}/invitations` (accept / decline one) and `/invitations` (all pending invitations).
import { useMemo, useState, type FormEvent } from "react";
import { Link, Navigate, useNavigate, useParams } from "react-router-dom";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, NotFoundView, RelTime, Spinner } from "../../components/common";
import type { RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { ra, type Collaborator, type CustomRole, type Invitation, type Team } from "./api";
import { Pill, SectionHead, Status, UserInput, useAction } from "./ui";

export const BASE_ROLES = [
  { value: "read", label: "Read", desc: "Recommended for non-code contributors who want to view or discuss your project" },
  { value: "triage", label: "Triage", desc: "Recommended for contributors who need to manage issues and pull requests without write access" },
  { value: "write", label: "Write", desc: "Recommended for contributors who actively push to your project" },
  { value: "maintain", label: "Maintain", desc: "Recommended for project managers who need to manage the repository without access to sensitive or destructive actions" },
  { value: "admin", label: "Admin", desc: "Recommended for people who need full access to the project, including sensitive and destructive actions like managing security or deleting a repository" },
];
/** Team API permission names (`pull`/`push`) ↔ role names. */
const teamPerm = (role: string) => ({ read: "pull", write: "push" } as Record<string, string>)[role] || role;
const roleOfPerm = (p: string | undefined) => ({ pull: "read", push: "write" } as Record<string, string>)[p || ""] || p || "read";
const roleLabel = (r: string) => BASE_ROLES.find((b) => b.value === r)?.label || r;

function RoleSelect({ id, value, onChange, custom, disabled, label }: {
  id: string; value: string; onChange: (v: string) => void; custom: CustomRole[]; disabled?: boolean; label: string;
}) {
  return (
    <select id={id} className="input input-auto input-sm" value={value} disabled={disabled} aria-label={label} onChange={(e) => onChange(e.target.value)}>
      {BASE_ROLES.map((r) => <option key={r.value} value={r.value}>{r.label}</option>)}
      {custom.length > 0 && (
        <optgroup label="Custom roles">
          {custom.map((c) => <option key={c.id} value={c.name}>{c.name}</option>)}
        </optgroup>
      )}
    </select>
  );
}

export function AccessSettings({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Collaborators and teams · ${ctx.repo.full_name}`);
  const isOrg = ctx.repo.owner.type === "Organization";
  const data = useAsync(async () => {
    const [collabs, invites, teams, custom] = await Promise.all([
      ra.collaborators(ctx.owner, ctx.name, isOrg ? "direct" : "all"),
      ra.repoInvitations(ctx.owner, ctx.name).catch(() => [] as Invitation[]),
      isOrg ? ra.repoTeams(ctx.owner, ctx.name).catch(() => [] as Team[]) : Promise.resolve([] as Team[]),
      isOrg ? ra.customRoles(ctx.owner).catch(() => [] as CustomRole[]) : Promise.resolve([] as CustomRole[]),
    ]);
    return { collabs: collabs.filter((c) => c.login !== ctx.owner), invites, teams, custom };
  }, [ctx.owner, ctx.name]);
  const [filter, setFilter] = useState("");
  const [adding, setAdding] = useState<"" | "people" | "team">("");
  const act = useAction();

  return (
    <div className="settings ra-access">
      <SectionHead title="Collaborators and teams" />
      <div className="ra-summary">
        <div className="box box-body ra-summary-card">
          <div className="small muted">{ctx.repo.private ? "Private repository" : "Public repository"}</div>
          <div className="small">{ctx.repo.private ? "Only those with access to this repository can view it." : "This repository is public and visible to anyone."}</div>
        </div>
        <div className="box box-body ra-summary-card">
          <div className="small muted">Direct access</div>
          <div className="small" data-testid="access-count">
            {data.data ? `${data.data.collabs.length} ${data.data.collabs.length === 1 ? "collaborator has" : "collaborators have"} access to this repository.` : "…"}
            {data.data && data.data.invites.length > 0 && ` ${data.data.invites.length} pending.`}
            {data.data && data.data.teams.length > 0 && ` ${data.data.teams.length} ${data.data.teams.length === 1 ? "team" : "teams"}.`}
          </div>
        </div>
      </div>

      <section className="box">
        <div className="box-header ra-box-header">
          <h3 className="h5">Manage access</h3>
          <div className="ra-head-actions">
            {isOrg && <button type="button" className="btn btn-sm" onClick={() => setAdding(adding === "team" ? "" : "team")}>Add teams</button>}
            <button type="button" className="btn btn-sm btn-primary" onClick={() => setAdding(adding === "people" ? "" : "people")}>Add people</button>
          </div>
        </div>
        {adding === "people" && data.data && (
          <AddPeople ctx={ctx} custom={data.data.custom} onDone={() => { setAdding(""); data.reload(); }} onCancel={() => setAdding("")} />
        )}
        {adding === "team" && data.data && (
          <AddTeam ctx={ctx} custom={data.data.custom} existing={data.data.teams} onDone={() => { setAdding(""); data.reload(); }} onCancel={() => setAdding("")} />
        )}
        <div className="box-body ra-filter">
          <input className="input" placeholder="Find people or a team…" aria-label="Find a collaborator" value={filter} onChange={(e) => setFilter(e.target.value)} />
        </div>
        <Status state={act.state} />
        <Async state={data}>
          {(d) => {
            const f = filter.trim().toLowerCase();
            const collabs = d.collabs.filter((c) => !f || c.login.toLowerCase().includes(f));
            const invites = d.invites.filter((i) => !f || (i.invitee?.login || "").toLowerCase().includes(f));
            const teams = d.teams.filter((t) => !f || t.name.toLowerCase().includes(f) || t.slug.includes(f));
            if (!collabs.length && !invites.length && !teams.length)
              return <Blankslate title={f ? "No matches" : "You haven't invited any collaborators yet"}>{f ? "" : "Invite collaborators to give them access to this repository."}</Blankslate>;
            return (
              <ul className="ra-list" data-testid="access-list">
                {teams.map((t) => (
                  <TeamRow key={`t${t.id}`} ctx={ctx} team={t} custom={d.custom} act={act} onChange={data.reload} />
                ))}
                {collabs.map((c) => (
                  <CollaboratorRow key={`c${c.id}`} ctx={ctx} c={c} custom={d.custom} act={act} onChange={data.reload} />
                ))}
                {invites.map((i) => (
                  <InviteRow key={`i${i.id}`} ctx={ctx} inv={i} act={act} onChange={data.reload} />
                ))}
              </ul>
            );
          }}
        </Async>
      </section>
    </div>
  );
}

type Act = ReturnType<typeof useAction>;

function CollaboratorRow({ ctx, c, custom, act, onChange }: { ctx: RepoCtx; c: Collaborator; custom: CustomRole[]; act: Act; onChange: () => void }) {
  const role = c.role_name || (c.permissions?.admin ? "admin" : c.permissions?.maintain ? "maintain" : c.permissions?.push ? "write" : c.permissions?.triage ? "triage" : "read");
  return (
    <li className="ra-row" data-testid={`collab-${c.login}`}>
      <Avatar user={c} size={32} />
      <div className="ra-row-main">
        <Link to={`/${c.login}`} className="ra-strong">{c.login}</Link>
        <div className="small muted">Collaborator</div>
      </div>
      <div className="ra-row-actions">
        <RoleSelect id={`role-${c.login}`} label={`Role of ${c.login}`} value={role} custom={custom} disabled={act.busy}
          onChange={async (v) => {
            await act.run(() => ra.addCollaborator(ctx.owner, ctx.name, c.login, v), `${c.login}'s role changed to ${roleLabel(v)}.`);
            onChange();
          }} />
        <button type="button" className="btn btn-sm btn-danger" disabled={act.busy} aria-label={`Remove ${c.login}`}
          onClick={async () => {
            if (!confirm(`Remove ${c.login} from ${ctx.repo.full_name}?`)) return;
            await act.run(() => ra.removeCollaborator(ctx.owner, ctx.name, c.login), `Removed ${c.login} from this repository.`);
            onChange();
          }}>Remove</button>
      </div>
    </li>
  );
}

function TeamRow({ ctx, team, custom, act, onChange }: { ctx: RepoCtx; team: Team; custom: CustomRole[]; act: Act; onChange: () => void }) {
  const role = team.role_name && team.role_name !== "custom" ? roleOfPerm(team.role_name) : roleOfPerm(team.permission);
  return (
    <li className="ra-row" data-testid={`team-${team.slug}`}>
      <span className="ra-team-icon" aria-hidden="true">{team.name.slice(0, 1).toUpperCase()}</span>
      <div className="ra-row-main">
        <Link to={`/orgs/${ctx.owner}/teams/${team.slug}`} className="ra-strong">{ctx.owner}/{team.slug}</Link>
        <div className="small muted">Team</div>
      </div>
      <div className="ra-row-actions">
        <RoleSelect id={`team-role-${team.slug}`} label={`Role of team ${team.name}`} value={role} custom={custom} disabled={act.busy}
          onChange={async (v) => {
            await act.run(() => ra.setTeamRepo(ctx.owner, team.slug, ctx.owner, ctx.name, teamPerm(v)), `Team ${team.name}'s role changed to ${roleLabel(v)}.`);
            onChange();
          }} />
        <button type="button" className="btn btn-sm btn-danger" disabled={act.busy} aria-label={`Remove team ${team.name}`}
          onClick={async () => {
            if (!confirm(`Remove team ${team.name} from ${ctx.repo.full_name}?`)) return;
            await act.run(() => ra.removeTeamRepo(ctx.owner, team.slug, ctx.owner, ctx.name), `Removed team ${team.name}.`);
            onChange();
          }}>Remove</button>
      </div>
    </li>
  );
}

function InviteRow({ ctx, inv, act, onChange }: { ctx: RepoCtx; inv: Invitation; act: Act; onChange: () => void }) {
  const login = inv.invitee?.login || "unknown";
  const role = roleOfPerm(inv.permissions);
  return (
    <li className="ra-row" data-testid={`invite-${login}`}>
      <Avatar user={inv.invitee} size={32} />
      <div className="ra-row-main">
        <span className="ra-strong">{login}</span> <Pill kind="warn">{inv.expired ? "Invite expired" : "Pending invite"}</Pill>
        <div className="small muted">Invited <RelTime iso={inv.created_at} /> by {inv.inviter?.login}</div>
      </div>
      <div className="ra-row-actions">
        <select className="input input-auto input-sm" value={role} aria-label={`Invitation role of ${login}`} disabled={act.busy}
          onChange={async (e) => {
            await act.run(() => ra.updateInvitation(ctx.owner, ctx.name, inv.id, e.target.value), `Invitation for ${login} updated.`);
            onChange();
          }}>
          {BASE_ROLES.map((r) => <option key={r.value} value={r.value}>{r.label}</option>)}
        </select>
        <button type="button" className="btn btn-sm" disabled={act.busy}
          onClick={async () => {
            await act.run(() => ra.deleteInvitation(ctx.owner, ctx.name, inv.id), `Invitation for ${login} cancelled.`);
            onChange();
          }}>Cancel invite</button>
      </div>
    </li>
  );
}

function AddPeople({ ctx, custom, onDone, onCancel }: { ctx: RepoCtx; custom: CustomRole[]; onDone: () => void; onCancel: () => void }) {
  const [login, setLogin] = useState("");
  const [role, setRole] = useState("write");
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const ok = await act.run(async () => {
      await ra.user(login.trim());
      return (await ra.addCollaborator(ctx.owner, ctx.name, login.trim(), role)) ?? true;
    });
    if (ok) onDone();
  };
  return (
    <form className="box-body ra-add" onSubmit={submit} aria-label="Add people">
      <h4 className="h5">Add a collaborator to {ctx.name}</h4>
      <Status state={act.state} />
      <div className="form-group">
        <label htmlFor="add-collab">Username</label>
        <UserInput id="add-collab" value={login} onChange={setLogin} />
      </div>
      <fieldset className="form-group">
        <legend>Choose a role</legend>
        {BASE_ROLES.map((r) => (
          <label key={r.value} className="radio-row">
            <input type="radio" name="add-role" value={r.value} checked={role === r.value} onChange={() => setRole(r.value)} />
            <span><strong>{r.label}</strong><span className="ra-check-desc small muted">{r.desc}</span></span>
          </label>
        ))}
        {custom.map((r) => (
          <label key={r.id} className="radio-row">
            <input type="radio" name="add-role" value={r.name} checked={role === r.name} onChange={() => setRole(r.name)} />
            <span><strong>{r.name}</strong><span className="ra-check-desc small muted">{r.description || `Custom role based on ${r.base_role}`}</span></span>
          </label>
        ))}
      </fieldset>
      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={act.busy || !login.trim()}>Add {login.trim() || "collaborator"} to this repository</button>
        <button type="button" className="btn" onClick={onCancel}>Cancel</button>
      </div>
    </form>
  );
}

function AddTeam({ ctx, custom, existing, onDone, onCancel }: { ctx: RepoCtx; custom: CustomRole[]; existing: Team[]; onDone: () => void; onCancel: () => void }) {
  const teams = useAsync(() => ra.orgTeams(ctx.owner), [ctx.owner]);
  const [slug, setSlug] = useState("");
  const [role, setRole] = useState("read");
  const act = useAction();
  const avail = useMemo(() => (teams.data || []).filter((t) => !existing.some((e) => e.id === t.id)), [teams.data, existing]);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const ok = await act.run(() => ra.setTeamRepo(ctx.owner, slug, ctx.owner, ctx.name, teamPerm(role)).then(() => true));
    if (ok) onDone();
  };
  return (
    <form className="box-body ra-add" onSubmit={submit} aria-label="Add teams">
      <h4 className="h5">Add a team to {ctx.name}</h4>
      <Status state={act.state} />
      {teams.loading ? <Spinner /> : avail.length === 0 ? (
        <p className="muted">No other teams in {ctx.owner}. <Link to={`/orgs/${ctx.owner}/teams`}>Create a team</Link></p>
      ) : (
        <>
          <div className="form-group">
            <label htmlFor="add-team">Team</label>
            <select id="add-team" className="input" value={slug} onChange={(e) => setSlug(e.target.value)} required>
              <option value="">Select a team</option>
              {avail.map((t) => <option key={t.id} value={t.slug}>{t.name}</option>)}
            </select>
          </div>
          <div className="form-group">
            <label htmlFor="add-team-role">Role</label>
            <RoleSelect id="add-team-role" label="Team role" value={role} onChange={setRole} custom={custom} />
          </div>
        </>
      )}
      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={act.busy || !slug}>Add team</button>
        <button type="button" className="btn" onClick={onCancel}>Cancel</button>
      </div>
    </form>
  );
}

// ---------- invitee side ----------

/** `/{o}/{r}/invitations`: accept or decline the viewer's invitation to this repository. */
export function RepoInvitationPage() {
  const { owner = "", repo = "" } = useParams();
  const { user, loading } = useAuth();
  const nav = useNavigate();
  useTitle(`Invitation · ${owner}/${repo}`);
  const st = useAsync(() => (user ? ra.myInvitations() : Promise.resolve([] as Invitation[])), [user?.login]);
  const act = useAction();
  if (loading) return <Spinner />;
  if (!user) return <Navigate to={`/login?return_to=${encodeURIComponent(`/${owner}/${repo}/invitations`)}`} replace />;
  return (
    <div className="container narrow ra-invite-page">
      <Async state={st}>
        {(list) => {
          const inv = list.find((i) => i.repository.full_name.toLowerCase() === `${owner}/${repo}`.toLowerCase());
          if (!inv) return <NotFoundView />;
          return (
            <div className="box box-body ra-invite" data-testid="invitation">
              <Avatar user={inv.inviter} size={48} />
              <h1 className="h3"><strong>{inv.inviter?.login}</strong> invited you to collaborate on <strong>{inv.repository.full_name}</strong></h1>
              <p className="muted">You'll get <strong>{roleLabel(roleOfPerm(inv.permissions))}</strong> access. Invited <RelTime iso={inv.created_at} />.</p>
              <Status state={act.state} />
              <div className="form-actions ra-center">
                <button type="button" className="btn btn-primary" disabled={act.busy}
                  onClick={async () => {
                    const ok = await act.run(() => ra.acceptInvitation(inv.id).then(() => true));
                    if (ok) nav(`/${inv.repository.full_name}`);
                  }}>Accept invitation</button>
                <button type="button" className="btn" disabled={act.busy}
                  onClick={async () => {
                    const ok = await act.run(() => ra.declineInvitation(inv.id).then(() => true));
                    if (ok) nav("/invitations");
                  }}>Decline</button>
              </div>
              <p className="small muted">Owners of {inv.repository.full_name} will be able to see your public profile information and whether you accepted.</p>
            </div>
          );
        }}
      </Async>
    </div>
  );
}

/** `/invitations`: all of the viewer's pending repository invitations. */
export function InvitationsPage() {
  const { user, loading } = useAuth();
  useTitle("Repository invitations");
  const st = useAsync(() => (user ? ra.myInvitations() : Promise.resolve([] as Invitation[])), [user?.login]);
  const act = useAction();
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Finvitations" replace />;
  return (
    <div className="container narrow ra-invites-page">
      <SectionHead title="Repository invitations">Invitations to collaborate on repositories. Invitations expire after 7 days.</SectionHead>
      <Status state={act.state} />
      <Async state={st}>
        {(list) =>
          list.length === 0 ? (
            <div className="box"><Blankslate title="No pending invitations">When someone invites you to a repository, it shows up here.</Blankslate></div>
          ) : (
            <ul className="box ra-list" data-testid="my-invitations">
              {list.map((inv) => (
                <li key={inv.id} className="ra-row" data-testid={`my-invite-${inv.repository.full_name}`}>
                  <Avatar user={inv.inviter} size={32} />
                  <div className="ra-row-main">
                    <Link className="ra-strong" to={`/${inv.repository.full_name}/invitations`}>{inv.repository.full_name}</Link>
                    <div className="small muted">{inv.inviter?.login} invited you <RelTime iso={inv.created_at} /> · {roleLabel(roleOfPerm(inv.permissions))}</div>
                  </div>
                  <div className="ra-row-actions">
                    <button type="button" className="btn btn-sm btn-primary" disabled={act.busy}
                      onClick={async () => { await act.run(() => ra.acceptInvitation(inv.id), `You now have access to ${inv.repository.full_name}.`); st.reload(); }}>Accept</button>
                    <button type="button" className="btn btn-sm" disabled={act.busy}
                      onClick={async () => { await act.run(() => ra.declineInvitation(inv.id), `Declined the invitation to ${inv.repository.full_name}.`); st.reload(); }}>Decline</button>
                  </div>
                </li>
              ))}
            </ul>
          )
        }
      </Async>
    </div>
  );
}

/** Dashboard sidebar: pending invitations (hidden when there are none). */
export function PendingInvitationsBox() {
  const { user } = useAuth();
  const st = useAsync(() => (user ? ra.myInvitations().catch(() => [] as Invitation[]) : Promise.resolve([] as Invitation[])), [user?.login]);
  if (!st.data?.length) return null;
  return (
    <div className="ra-side-invites" data-testid="pending-invitations">
      <h2 className="h5">Pending invitations</h2>
      <ul>
        {st.data.slice(0, 5).map((i) => (
          <li key={i.id}><Link to={`/${i.repository.full_name}/invitations`}>{i.repository.full_name}</Link></li>
        ))}
      </ul>
      <Link className="small" to="/invitations">View all invitations</Link>
    </div>
  );
}


// The viewer's side of organizations: user settings "Organizations" (/settings/organizations),
// the invitation page (/orgs/:org/invitation) and the dashboard invitation notice.
import { useState } from "react";
import { Link, Navigate, useNavigate, useParams } from "react-router-dom";
import type { User } from "../../api";
import type { UserSettingsCtx } from "../types";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, ErrorBox, Flash, Spinner } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import { orgApi, type Membership } from "./api";
import { roleLabel, useAction, WithOrg } from "./common";

export function OrganizationsSettings({ ctx }: { ctx: UserSettingsCtx }) {
  const me = ctx.user;
  useTitle("Organizations");
  const data = useAsync(async () => {
    const all = await orgApi.myMemberships();
    const pub = await Promise.all(all.filter((m) => m.state === "active").map(async (m) => [m.organization.login, await orgApi.isPublicMember(m.organization.login, me.login)] as const));
    return { all, pub: new Map(pub) };
  }, [me.login]);
  const act = useAction();
  const [notice, setNotice] = useState("");
  const run = async (fn: () => Promise<unknown>, msg: string) => {
    setNotice("");
    if (await act.run(fn)) { setNotice(msg); data.reload(); }
  };
  return (
    <div className="settings">
      <div className="toolbar">
        <h2 className="h3">Organizations</h2>
        <span className="spacer" />
        <Link to="/organizations/new" className="btn btn-sm btn-primary" data-testid="new-org">New organization</Link>
      </div>
      {notice && <Flash kind="success">{notice}</Flash>}
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(d) => {
          const pending = d.all.filter((m) => m.state === "pending");
          const active = d.all.filter((m) => m.state === "active");
          return (
            <>
              {pending.length > 0 && (
                <>
                  <h3 className="h5">Pending invitations</h3>
                  <ul className="box list" data-testid="my-org-invitations">
                    {pending.map((m) => (
                      <li key={m.organization.login} className="list-row org-person" data-testid={`my-invite-${m.organization.login}`}>
                        <Avatar user={m.organization} size={32} />
                        <div className="grow">
                          <Link to={`/orgs/${m.organization.login}/invitation`}><strong>{m.organization.login}</strong></Link>
                          <div className="muted small">You've been invited to join as {m.role === "admin" ? "an owner" : "a member"}.</div>
                        </div>
                        <button type="button" className="btn btn-sm btn-primary" disabled={act.busy}
                          onClick={() => run(() => orgApi.acceptInvitation(m.organization.login), `You are now a member of ${m.organization.login}.`)}>Accept</button>
                        <button type="button" className="btn btn-sm" disabled={act.busy}
                          onClick={() => run(() => orgApi.declineInvitation(m.organization.login), `Declined the invitation to ${m.organization.login}.`)}>Decline</button>
                      </li>
                    ))}
                  </ul>
                </>
              )}
              {active.length === 0 ? (
                <Blankslate title="You are not a member of any organizations.">
                  <Link to="/organizations/new">Create an organization</Link> to collaborate with others.
                </Blankslate>
              ) : (
                <ul className="box list" data-testid="my-orgs">
                  {active.map((m) => (
                    <OrgRow key={m.organization.login} m={m} me={me} isPublic={!!d.pub.get(m.organization.login)} busy={act.busy}
                      onPublicize={(p) => run(() => (p ? orgApi.publicize(m.organization.login, me.login) : orgApi.conceal(m.organization.login, me.login)),
                        p ? `Your membership in ${m.organization.login} is now public.` : `Your membership in ${m.organization.login} is now private.`)}
                      onLeave={() => confirm(`Leave ${m.organization.login}?`) && run(() => orgApi.removeMember(m.organization.login, me.login), `You left ${m.organization.login}.`)} />
                  ))}
                </ul>
              )}
            </>
          );
        }}
      </Async>
    </div>
  );
}

function OrgRow({ m, me, isPublic, busy, onPublicize, onLeave }: {
  m: Membership; me: User; isPublic: boolean; busy: boolean; onPublicize: (p: boolean) => void; onLeave: () => void;
}) {
  const o = m.organization.login;
  return (
    <li className="list-row org-person" data-testid={`my-org-${o}`}>
      <Avatar user={m.organization} size={32} />
      <div className="grow">
        <Link to={`/${o}`}><strong>{o}</strong></Link> <span className="badge">{roleLabel(m.role)}</span>
        <div className="muted small">{isPublic ? "Public membership" : "Private membership"}</div>
      </div>
      {m.role === "admin" && <Link to={`/organizations/${o}/settings`} className="btn btn-sm">Settings</Link>}
      <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onPublicize(!isPublic)} data-testid={`publicize-${o}`}>
        {isPublic ? "Make private" : "Make public"}
      </button>
      <button type="button" className="btn btn-sm btn-danger-outline" disabled={busy} onClick={onLeave} data-testid={`leave-${o}`}
        title={`Leave ${o} as ${me.login}`}>Leave</button>
    </li>
  );
}

/** /orgs/:org/invitation: accept or decline an invitation. */
export function InvitationPage() {
  const { org = "" } = useParams();
  const { user, loading } = useAuth();
  const nav = useNavigate();
  const act = useAction();
  useTitle(`Invitation · ${org}`);
  if (loading) return <Spinner />;
  if (!user) return <Navigate to={`/login?return_to=${encodeURIComponent(location.pathname)}`} replace />;
  return (
    <WithOrg login={org}>
      {(st) => (
        <div className="container narrow org-invitation">
          <div className="box box-body org-invitation-box">
            <Avatar user={st.org} size={96} />
            {st.membership?.state === "pending" ? (
              <>
                <h1 className="h3">You've been invited to the <strong>{st.org.login}</strong> organization!</h1>
                <p className="muted">You will join as {st.membership.role === "admin" ? "an owner" : "a member"}. Owners of the organization
                  will be able to see your membership and your access to its repositories.</p>
                {act.error !== undefined && <ErrorBox error={act.error} />}
                <div className="form-actions org-invitation-actions">
                  <button type="button" className="btn btn-primary" disabled={act.busy} data-testid="join-org"
                    onClick={async () => { if (await act.run(() => orgApi.acceptInvitation(st.org.login))) nav(`/${st.org.login}`); }}>
                    Join {st.org.login}
                  </button>
                  <button type="button" className="btn" disabled={act.busy} data-testid="decline-org"
                    onClick={async () => { if (await act.run(() => orgApi.declineInvitation(st.org.login))) nav("/settings/organizations"); }}>
                    Decline
                  </button>
                </div>
              </>
            ) : st.membership?.state === "active" ? (
              <>
                <h1 className="h3">You are already a member of {st.org.login}.</h1>
                <Link to={`/${st.org.login}`} className="btn">Go to {st.org.login}</Link>
              </>
            ) : (
              <>
                <h1 className="h3">No pending invitation</h1>
                <p className="muted">There is no pending invitation to {st.org.login} for {user.login}. It may have expired or been cancelled.</p>
              </>
            )}
          </div>
        </div>
      )}
    </WithOrg>
  );
}

/** Dashboard sidebar (`home.sidebar` slot): pending organization invitations. */
export function PendingOrgInvites({ user }: { user: User }) {
  const data = useAsync(() => orgApi.myMemberships("pending").catch(() => [] as Membership[]), [user.login]);
  if (!data.data || data.data.length === 0) return null;
  return (
    <div className="sidebar-section" data-testid="home-org-invites">
      <h2 className="h5">Organization invitations</h2>
      <ul className="list small">
        {data.data.map((m) => (
          <li key={m.organization.login} className="list-row">
            <Avatar user={m.organization} size={20} />
            <Link to={`/orgs/${m.organization.login}/invitation`}>{m.organization.login}</Link> invited you
          </li>
        ))}
      </ul>
    </div>
  );
}

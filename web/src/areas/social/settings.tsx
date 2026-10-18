// User settings sections: public profile, account, appearance, emails, password, GPG and SSH
// signing keys, blocked users.
import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { Link } from "react-router-dom";
import type { UserSettingsCtx } from "../types";
import { useAuth } from "../../auth";
import { applyTheme } from "../../components/Layout";
import { Async, Avatar, Blankslate, ErrorBox, Flash, RelTime } from "../../components/common";
import { KeyIcon } from "../../components/icons";
import { useAsync, useTitle } from "../../hooks";
import { sapi, type Email, type GpgKey, type Me, type SigningKey } from "./api";
import { BlockIcon, MailIcon } from "./icons";

function Section({ title, children, desc }: { title: string; desc?: ReactNode; children: ReactNode }) {
  useTitle(title);
  return (
    <div className="social-settings">
      <h1 className="h2 settings-title">{title}</h1>
      {desc && <p className="muted">{desc}</p>}
      {children}
    </div>
  );
}

/** Run an async action with busy/error/success state. */
function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const [done, setDone] = useState("");
  const run = async (fn: () => Promise<unknown>, ok = "") => {
    setBusy(true);
    setError(undefined);
    setDone("");
    try {
      await fn();
      setDone(ok);
      return true;
    } catch (e) {
      setError(e);
      return false;
    } finally {
      setBusy(false);
    }
  };
  const flash = <>{error !== undefined && <ErrorBox error={error} />}{done && <Flash kind="success">{done}</Flash>}</>;
  return { busy, run, flash };
}

// ---------- Public profile ----------

export function ProfileSettings({ ctx }: { ctx: UserSettingsCtx }) {
  const { refresh } = useAuth();
  const me = useAsync(() => sapi.me(), [ctx.user.login]);
  const emails = useAsync(() => sapi.emails().catch(() => [] as Email[]), [ctx.user.login]);
  return (
    <Section title="Public profile">
      <Async state={me}>{(u) => <ProfileForm u={u} emails={emails.data || []} onSaved={() => { void refresh(); me.reload(); }} />}</Async>
      <SocialAccounts />
    </Section>
  );
}

function ProfileForm({ u, emails, onSaved }: { u: Me; emails: Email[]; onSaved: () => void }) {
  const [f, setF] = useState({
    name: u.name || "", email: u.email || "", bio: u.bio || "", blog: u.blog || "", company: u.company || "",
    location: u.location || "", twitter_username: u.twitter_username || "", hireable: !!u.hireable,
  });
  const a = useAction();
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void a.run(async () => {
      await sapi.updateMe({ ...f, email: f.email || undefined });
      onSaved();
    }, "Profile updated successfully.");
  };
  const field = (k: keyof typeof f, label: string, hint?: string, extra?: Record<string, string>) => (
    <div className="form-group">
      <label htmlFor={`pf-${k}`}>{label}</label>
      <input id={`pf-${k}`} className="input" value={String(f[k])} onChange={set(k)} {...extra} />
      {hint && <p className="muted small">{hint}</p>}
    </div>
  );
  return (
    <form className="profile-form" onSubmit={submit} data-testid="profile-form">
      <div className="profile-form-grid">
        <div>
          {a.flash}
          {field("name", "Name", "Your name may appear around forge where you contribute or are mentioned.")}
          <div className="form-group">
            <label htmlFor="pf-email">Public email</label>
            <select id="pf-email" className="input" value={f.email} onChange={set("email")}>
              {!emails.some((e) => e.email === f.email) && <option value={f.email}>{f.email || "Select a verified email to display"}</option>}
              {emails.filter((e) => e.verified).map((e) => <option key={e.email} value={e.email}>{e.email}</option>)}
            </select>
            <p className="muted small">You can manage verified email addresses in your <Link to="/settings/emails">email settings</Link>.</p>
          </div>
          <div className="form-group">
            <label htmlFor="pf-bio">Bio</label>
            <textarea id="pf-bio" className="input" rows={3} value={f.bio} onChange={set("bio")} placeholder="Tell us a little bit about yourself" />
          </div>
          {field("blog", "URL", undefined, { type: "text", placeholder: "https://example.com" })}
          {field("twitter_username", "Twitter username", undefined, { placeholder: "without the @" })}
          {field("company", "Company")}
          {field("location", "Location")}
          <label className="check-row">
            <input type="checkbox" checked={f.hireable} onChange={(e) => setF({ ...f, hireable: e.target.checked })} data-testid="pf-hireable" />
            <span><strong>Available for hire</strong><br /><span className="muted small">Show an "Available for hire" note on your profile.</span></span>
          </label>
          <button type="submit" className="btn btn-primary" disabled={a.busy} data-testid="profile-save">{a.busy ? "Saving…" : "Update profile"}</button>
        </div>
        <div className="profile-form-avatar">
          <span className="label-text">Profile picture</span>
          <Avatar user={u} size={160} />
        </div>
      </div>
    </form>
  );
}

function SocialAccounts() {
  const st = useAsync(() => sapi.socialAccounts(), []);
  const [url, setUrl] = useState("");
  const a = useAction();
  return (
    <section className="box social-accounts" data-testid="social-accounts">
      <div className="box-header"><h2 className="h5">Social accounts</h2></div>
      <div className="box-body">
        {a.flash}
        <Async state={st}>
          {(list) => (
            <ul className="list-plain">
              {list.length === 0 && <li className="muted small">No social accounts yet.</li>}
              {list.map((s) => (
                <li key={s.url} className="social-row" data-testid="social-row">
                  <span className="badge">{s.provider}</span> <a href={s.url} rel="nofollow noopener">{s.url}</a>
                  <button type="button" className="btn btn-sm btn-danger" disabled={a.busy} onClick={() => a.run(async () => { await sapi.deleteSocialAccount(s.url); st.reload(); })}
                    data-testid="social-remove">Remove</button>
                </li>
              ))}
            </ul>
          )}
        </Async>
        {(st.data?.length || 0) < 4 && (
          <form className="input-group" onSubmit={(e) => { e.preventDefault(); void a.run(async () => { await sapi.addSocialAccount(url.trim()); setUrl(""); st.reload(); }); }}>
            <label htmlFor="social-url" className="sr-only">Link to social profile</label>
            <input id="social-url" className="input" placeholder="Link to social profile" value={url} onChange={(e) => setUrl(e.target.value)} />
            <button type="submit" className="btn" disabled={a.busy || !url.trim()} data-testid="social-add">Add</button>
          </form>
        )}
      </div>
    </section>
  );
}

// ---------- Account ----------

export function AccountSettings({ ctx }: { ctx: UserSettingsCtx }) {
  const { setUser, user } = useAuth();
  const [login, setLogin] = useState(ctx.user.login);
  const rename = useAction();
  const del = useAction();
  const exp = useAction();
  const [delLogin, setDelLogin] = useState("");
  const [delPw, setDelPw] = useState("");
  const migrations = useAsync(() => sapi.migrations(), [ctx.user.login]);
  useEffect(() => {
    const pending = (migrations.data || []).some((m) => m.state !== "exported" && m.state !== "failed");
    if (!pending) return;
    const t = setTimeout(migrations.reload, 2000);
    return () => clearTimeout(t);
  }, [migrations.data]);
  return (
    <Section title="Account">
      <section className="box">
        <div className="box-header"><h2 className="h5">Change username</h2></div>
        <form className="box-body" onSubmit={(e) => {
          e.preventDefault();
          if (!confirm("Changing your username can have unintended side effects. Links to your old profile will stop working. Continue?")) return;
          void rename.run(async () => {
            const u = await sapi.rename(login.trim());
            setUser({ ...(user || u), ...u });
          }, "Your username has been changed.");
        }}>
          {rename.flash}
          <div className="form-group">
            <label htmlFor="acct-login">Username</label>
            <input id="acct-login" className="input" value={login} onChange={(e) => setLogin(e.target.value)} data-testid="rename-input" />
          </div>
          <button type="submit" className="btn" disabled={rename.busy || !login.trim() || login === ctx.user.login} data-testid="rename-submit">Change username</button>
        </form>
      </section>

      <section className="box">
        <div className="box-header"><h2 className="h5">Export account data</h2></div>
        <div className="box-body">
          <p className="muted small">Export your repositories and their metadata (issues, pull requests, releases) as an archive you can download.</p>
          {exp.flash}
          <button type="button" className="btn" disabled={exp.busy} data-testid="export-start" onClick={() => exp.run(async () => {
            const repos = await sapi.myRepos();
            if (repos.length === 0) throw new Error("You don't own any repositories to export.");
            await sapi.startMigration(repos.map((r) => r.full_name));
            migrations.reload();
          }, "We're preparing your export. It will appear below when it's ready.")}>Start export</button>
          <Async state={migrations}>
            {(list) => list.length === 0 ? null : (
              <ul className="list-plain export-list" data-testid="export-list">
                {list.map((m) => (
                  <li key={m.id} className="export-row">
                    Export #{m.id} · <span className="badge" data-testid="export-state">{m.state}</span> · <RelTime iso={m.created_at} /> · {m.repositories?.length ?? 0} repositories{" "}
                    {m.state === "exported" && <a href={`/api/v3/user/migrations/${m.id}/archive`} className="btn btn-sm" data-testid="export-download">Download</a>}
                  </li>
                ))}
              </ul>
            )}
          </Async>
        </div>
      </section>

      <section className="box danger-zone">
        <div className="box-header"><h2 className="h5 c-danger">Delete account</h2></div>
        <form className="box-body" onSubmit={(e) => {
          e.preventDefault();
          if (!confirm("Are you sure you want to delete your account? This cannot be undone.")) return;
          void del.run(async () => {
            await sapi.deleteAccount(delLogin.trim(), delPw);
            // Full reload: the session is gone, start from the signed-out home page.
            window.location.assign("/");
          });
        }}>
          <p className="muted small">Once you delete your account, there is no going back. Your repositories, gists and all other data will be permanently removed.</p>
          {del.flash}
          {ctx.user.site_admin && <Flash kind="warn">Site administrators can't delete their own account.</Flash>}
          <div className="form-group">
            <label htmlFor="del-login">Your username</label>
            <input id="del-login" className="input" value={delLogin} onChange={(e) => setDelLogin(e.target.value)} autoComplete="off" data-testid="delete-login" />
          </div>
          <div className="form-group">
            <label htmlFor="del-pw">Confirm your password</label>
            <input id="del-pw" type="password" className="input" value={delPw} onChange={(e) => setDelPw(e.target.value)} autoComplete="current-password" data-testid="delete-password" />
          </div>
          <button type="submit" className="btn btn-danger" disabled={del.busy || delLogin.toLowerCase() !== ctx.user.login.toLowerCase() || !delPw}
            data-testid="delete-account">Delete your account</button>
        </form>
      </section>
    </Section>
  );
}

// ---------- Appearance ----------

type Theme = "auto" | "light" | "dark";
function readTheme(): Theme {
  try {
    const t = localStorage.getItem("forge-theme");
    return t === "light" || t === "dark" ? t : "auto";
  } catch {
    return "auto";
  }
}

export function AppearanceSettings() {
  const [theme, setTheme] = useState<Theme>(readTheme);
  // Keep in sync with the header toggle.
  useEffect(() => {
    const h = () => setTheme(readTheme());
    window.addEventListener("forge-theme", h);
    return () => window.removeEventListener("forge-theme", h);
  }, []);
  const pick = (t: Theme) => {
    setTheme(t);
    try {
      if (t === "auto") localStorage.removeItem("forge-theme");
      else localStorage.setItem("forge-theme", t);
    } catch {
      /* storage unavailable */
    }
    applyTheme(t);
    window.dispatchEvent(new Event("forge-theme"));
  };
  const opts: [Theme, string, string][] = [
    ["auto", "Sync with system", "forge will match your system's light or dark setting."],
    ["light", "Light default", "Always use the light theme."],
    ["dark", "Dark default", "Always use the dark theme."],
  ];
  return (
    <Section title="Appearance" desc="Choose how forge looks to you. The setting is saved in this browser and matches the theme switch in the header.">
      <fieldset className="form-group theme-picker">
        <legend>Theme mode</legend>
        {opts.map(([k, l, d]) => (
          <label key={k} className={"theme-card" + (theme === k ? " selected" : "")} data-testid={`theme-${k}`}>
            <input type="radio" name="theme" value={k} checked={theme === k} onChange={() => pick(k)} />
            <span className={`theme-swatch theme-swatch-${k}`} aria-hidden="true" />
            <span><strong>{l}</strong><br /><span className="muted small">{d}</span></span>
          </label>
        ))}
      </fieldset>
    </Section>
  );
}

// ---------- Emails ----------

export function EmailSettings({ ctx }: { ctx: UserSettingsCtx }) {
  const { refresh } = useAuth();
  const st = useAsync(() => sapi.emails(), [ctx.user.login]);
  const [email, setEmail] = useState("");
  const a = useAction();
  return (
    <Section title="Emails">
      {a.flash}
      <Async state={st}>
        {(list) => (
          <ul className="box list" data-testid="email-list">
            {list.map((e) => (
              <li key={e.email} className="list-row email-row" data-testid="email-row">
                <MailIcon />
                <div className="grow">
                  <strong data-testid="email-address">{e.email}</strong>{" "}
                  {e.primary && <span className="badge" data-testid="email-primary">Primary</span>}{" "}
                  {e.verified ? <span className="badge badge-added">Verified</span> : <span className="badge badge-warn">Unverified</span>}
                  {e.primary && <div className="muted small">This email will be used for account-related notifications and can also be used for password resets.</div>}
                </div>
                {!e.primary && (
                  <>
                    <button type="button" className="btn btn-sm" disabled={a.busy} data-testid="email-make-primary"
                      onClick={() => a.run(async () => { await sapi.updateMe({ email: e.email }); st.reload(); await refresh(); }, `${e.email} is now your primary email.`)}>Make primary</button>
                    <button type="button" className="btn btn-sm btn-danger" disabled={a.busy} aria-label={`Remove ${e.email}`} data-testid="email-remove"
                      onClick={() => a.run(async () => { await sapi.deleteEmail(e.email); st.reload(); })}>Remove</button>
                  </>
                )}
              </li>
            ))}
          </ul>
        )}
      </Async>
      <section className="box">
        <div className="box-header"><h2 className="h5">Add email address</h2></div>
        <form className="box-body input-group" onSubmit={(e) => { e.preventDefault(); void a.run(async () => { await sapi.addEmail(email.trim()); setEmail(""); st.reload(); }, "Email address added."); }}>
          <label htmlFor="new-email" className="sr-only">Email address</label>
          <input id="new-email" type="email" className="input" placeholder="Email address" value={email} onChange={(e) => setEmail(e.target.value)} data-testid="email-input" />
          <button type="submit" className="btn" disabled={a.busy || !email.trim()} data-testid="email-add">Add</button>
        </form>
      </section>
    </Section>
  );
}

// ---------- Password ----------

export function PasswordSettings() {
  const [cur, setCur] = useState("");
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const a = useAction();
  const mismatch = pw2.length > 0 && pw !== pw2;
  return (
    <Section title="Password and authentication">
      <section className="box">
        <div className="box-header"><h2 className="h5">Change password</h2></div>
        <form className="box-body" onSubmit={(e) => {
          e.preventDefault();
          void a.run(async () => { await sapi.changePassword(cur, pw); setCur(""); setPw(""); setPw2(""); }, "Password changed successfully.");
        }}>
          {a.flash}
          <div className="form-group">
            <label htmlFor="pw-old">Old password</label>
            <input id="pw-old" type="password" className="input" autoComplete="current-password" value={cur} onChange={(e) => setCur(e.target.value)} data-testid="pw-old" />
          </div>
          <div className="form-group">
            <label htmlFor="pw-new">New password</label>
            <input id="pw-new" type="password" className="input" autoComplete="new-password" value={pw} onChange={(e) => setPw(e.target.value)} data-testid="pw-new" />
            <p className="muted small">Make sure it's at least 8 characters.</p>
          </div>
          <div className="form-group">
            <label htmlFor="pw-new2">Confirm new password</label>
            <input id="pw-new2" type="password" className="input" autoComplete="new-password" value={pw2} aria-invalid={mismatch} onChange={(e) => setPw2(e.target.value)} data-testid="pw-confirm" />
            {mismatch && <p className="small c-danger">Passwords don't match.</p>}
          </div>
          <button type="submit" className="btn" disabled={a.busy || !cur || pw.length < 8 || pw !== pw2} data-testid="pw-submit">Update password</button>
        </form>
      </section>
      <section className="box">
        <div className="box-header"><h2 className="h5">Access tokens and SSH keys</h2></div>
        <div className="box-body small">
          Use <Link to="/settings/tokens">personal access tokens</Link> for git over HTTPS and the API, and <Link to="/settings/keys">SSH keys</Link> for git over SSH.
        </div>
      </section>
    </Section>
  );
}

// ---------- GPG and SSH signing keys ----------

export function SigningKeysSettings({ ctx }: { ctx: UserSettingsCtx }) {
  const gpg = useAsync(() => sapi.gpgKeys(), [ctx.user.login]);
  const ssh = useAsync(() => sapi.signingKeys(), [ctx.user.login]);
  const [gName, setGName] = useState("");
  const [gKey, setGKey] = useState("");
  const [sTitle, setSTitle] = useState("");
  const [sKey, setSKey] = useState("");
  const ga = useAction();
  const sa = useAction();
  return (
    <Section title="GPG and SSH signing keys" desc={<>Keys used to verify signed commits and tags. Your public GPG keys are published at <a href={`/api/v3/users/${ctx.user.login}/gpg_keys`}>/users/{ctx.user.login}/gpg_keys</a>.</>}>
      <section className="box" data-testid="gpg-keys">
        <div className="box-header"><h2 className="h5">GPG keys</h2></div>
        {ga.flash}
        <Async state={gpg}>
          {(keys) => keys.length === 0 ? <Blankslate title="There are no GPG keys associated with your account." icon={<KeyIcon size={24} />} /> : (
            <ul className="list">
              {keys.map((k: GpgKey) => (
                <li key={k.id} className="list-row token-row" data-testid="gpg-key-row">
                  <KeyIcon />
                  <div className="grow">
                    <strong>{k.name || "GPG key"}</strong>
                    <div className="small">Key ID: <code data-testid="gpg-key-id">{k.key_id}</code></div>
                    <div className="muted small">Email address: {k.emails.map((e) => e.email).join(", ") || "none"}</div>
                    {k.subkeys.length > 0 && <div className="muted small">Subkeys: {k.subkeys.map((s) => s.key_id).join(", ")}</div>}
                    <div className="muted small">Added <RelTime iso={k.created_at} />{k.expires_at && <> · expires <RelTime iso={k.expires_at} /></>}</div>
                  </div>
                  <button type="button" className="btn btn-sm btn-danger" data-testid="gpg-key-delete"
                    onClick={() => confirm("Are you sure you want to delete this GPG key?") && ga.run(async () => { await sapi.deleteGpgKey(k.id); gpg.reload(); })}>Delete</button>
                </li>
              ))}
            </ul>
          )}
        </Async>
        <form className="box-body" onSubmit={(e) => { e.preventDefault(); void ga.run(async () => { await sapi.addGpgKey({ name: gName.trim() || undefined, armored_public_key: gKey.trim() }); setGName(""); setGKey(""); gpg.reload(); }, "GPG key added."); }}>
          <h3 className="h5">Add new GPG key</h3>
          <div className="form-group">
            <label htmlFor="gpg-name">Title</label>
            <input id="gpg-name" className="input" value={gName} onChange={(e) => setGName(e.target.value)} data-testid="gpg-name" />
          </div>
          <div className="form-group">
            <label htmlFor="gpg-key">Key</label>
            <textarea id="gpg-key" className="input textarea mono" rows={6} value={gKey} onChange={(e) => setGKey(e.target.value)}
              placeholder="Begins with '-----BEGIN PGP PUBLIC KEY BLOCK-----'" data-testid="gpg-key-input" />
          </div>
          <button type="submit" className="btn btn-primary" disabled={ga.busy || !gKey.trim()} data-testid="gpg-add">Add GPG key</button>
        </form>
      </section>

      <section className="box" data-testid="ssh-signing-keys">
        <div className="box-header"><h2 className="h5">SSH signing keys</h2></div>
        {sa.flash}
        <Async state={ssh}>
          {(keys) => keys.length === 0 ? <Blankslate title="There are no SSH signing keys associated with your account." icon={<KeyIcon size={24} />} /> : (
            <ul className="list">
              {keys.map((k: SigningKey) => (
                <li key={k.id} className="list-row token-row" data-testid="signing-key-row">
                  <KeyIcon />
                  <div className="grow">
                    <strong>{k.title}</strong>
                    <div className="mono muted small key-summary">{k.key.split(/\s+/)[0]} …{(k.key.split(/\s+/)[1] || "").slice(-16)}</div>
                    <div className="muted small">Added <RelTime iso={k.created_at} /></div>
                  </div>
                  <button type="button" className="btn btn-sm btn-danger" data-testid="signing-key-delete"
                    onClick={() => confirm("Are you sure you want to delete this SSH signing key?") && sa.run(async () => { await sapi.deleteSigningKey(k.id); ssh.reload(); })}>Delete</button>
                </li>
              ))}
            </ul>
          )}
        </Async>
        <form className="box-body" onSubmit={(e) => { e.preventDefault(); void sa.run(async () => { await sapi.addSigningKey({ title: sTitle.trim(), key: sKey.trim() }); setSTitle(""); setSKey(""); ssh.reload(); }, "SSH signing key added."); }}>
          <h3 className="h5">Add new SSH signing key</h3>
          <div className="form-group">
            <label htmlFor="sk-title">Title</label>
            <input id="sk-title" className="input" value={sTitle} onChange={(e) => setSTitle(e.target.value)} data-testid="signing-title" />
          </div>
          <div className="form-group">
            <label htmlFor="sk-key">Key</label>
            <textarea id="sk-key" className="input textarea mono" rows={4} value={sKey} onChange={(e) => setSKey(e.target.value)}
              placeholder="Begins with 'ssh-ed25519', 'ssh-rsa', 'ecdsa-sha2-nistp256', …" data-testid="signing-key-input" />
          </div>
          <button type="submit" className="btn btn-primary" disabled={sa.busy || !sTitle.trim() || !sKey.trim()} data-testid="signing-add">Add SSH signing key</button>
        </form>
      </section>
    </Section>
  );
}

// ---------- Blocked users ----------

export function BlockedUsersSettings({ ctx }: { ctx: UserSettingsCtx }) {
  const st = useAsync(() => sapi.blocks(), [ctx.user.login]);
  const [login, setLogin] = useState("");
  const a = useAction();
  return (
    <Section title="Blocked users" desc="Blocking a user prevents them from following you, opening issues or pull requests and commenting in your repositories. Existing follows are removed.">
      {a.flash}
      <form className="input-group" onSubmit={(e) => { e.preventDefault(); void a.run(async () => { await sapi.block(login.trim()); setLogin(""); st.reload(); }, "User blocked."); }}>
        <label htmlFor="block-login" className="sr-only">Username</label>
        <input id="block-login" className="input" placeholder="Search by username" value={login} onChange={(e) => setLogin(e.target.value)} data-testid="block-input" />
        <button type="submit" className="btn" disabled={a.busy || !login.trim()} data-testid="block-submit"><BlockIcon /> Block user</button>
      </form>
      <Async state={st}>
        {(list) => list.length === 0 ? <Blankslate title="You have not blocked any users." /> : (
          <ul className="box list" data-testid="blocked-list">
            {list.map((u) => (
              <li key={u.id} className="list-row" data-testid="blocked-row">
                <Avatar user={u} size={20} /> <Link to={`/${u.login}`} className="grow">{u.login}</Link>
                <button type="button" className="btn btn-sm" disabled={a.busy} data-testid="unblock-button"
                  onClick={() => a.run(async () => { await sapi.unblock(u.login); st.reload(); }, `${u.login} has been unblocked.`)}>Unblock</button>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </Section>
  );
}


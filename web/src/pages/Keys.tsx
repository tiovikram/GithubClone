import { useState, type FormEvent } from "react";
import { Navigate } from "react-router-dom";
import { api, errorMessage, type SshKey } from "../api";
import { useAuth } from "../auth";
import { Async, Blankslate, CodeLine, ErrorBox, RelTime, Spinner } from "../components/common";
import { KeyIcon } from "../components/icons";
import { useAsync, useTitle } from "../hooks";
import { SettingsNav } from "./Tokens";

/** Fingerprint-ish summary for display: key type plus the tail of the base64 blob. */
function keySummary(k: string): string {
  const [type, blob] = k.trim().split(/\s+/);
  return blob ? `${type} …${blob.slice(-16)}` : type;
}

/** `/settings/keys` */
export function KeysPage() {
  const { user, loading } = useAuth();
  useTitle("SSH keys");
  const st = useAsync(() => (user ? api.sshKeys() : Promise.resolve([] as SshKey[])), [user?.login]);
  const [title, setTitle] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const [delErr, setDelErr] = useState("");
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Fsettings%2Fkeys" replace />;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const k = await api.addSshKey({ title: title.trim(), key: key.trim() });
      st.setData([...(st.data || []), k]);
      setTitle("");
      setKey("");
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };
  const remove = async (k: SshKey) => {
    if (!confirm(`Delete SSH key “${k.title || k.id}”?`)) return;
    setDelErr("");
    try {
      await api.deleteSshKey(k.id);
      st.setData((st.data || []).filter((x) => x.id !== k.id));
    } catch (e) {
      setDelErr(errorMessage(e));
    }
  };
  return (
    <div className="container settings-layout">
      <SettingsNav />
      <div className="settings-main">
        <h1 className="h2">SSH keys</h1>
        <p className="muted">
          SSH keys let you clone and push over SSH without a password. Your public keys are also published at{" "}
          <a href={`/${user.login}.keys`}>/{user.login}.keys</a>.
        </p>
        <section className="box">
          <div className="box-header"><h2 className="h5">Your keys</h2></div>
          {delErr && <ErrorBox error={new Error(delErr)} />}
          <Async state={st}>
            {(keys) =>
              keys.length === 0 ? (
                <Blankslate title="There are no SSH keys associated with your account" icon={<KeyIcon size={24} />} />
              ) : (
                <ul className="list" aria-label="SSH keys">
                  {keys.map((k) => (
                    <li key={k.id} className="list-row token-row" data-testid="ssh-key-row">
                      <KeyIcon />
                      <div className="grow">
                        <strong>{k.title || "(untitled)"}</strong>
                        <div className="mono muted small key-summary">{keySummary(k.key)}</div>
                        {k.created_at && <div className="muted small">Added <RelTime iso={k.created_at} /></div>}
                      </div>
                      <button type="button" className="btn btn-sm btn-danger" onClick={() => remove(k)}>Delete</button>
                    </li>
                  ))}
                </ul>
              )
            }
          </Async>
        </section>
        <section className="box">
          <div className="box-header"><h2 className="h5">Add new SSH key</h2></div>
          <form className="box-body" onSubmit={submit}>
            {error !== undefined && <ErrorBox error={error} />}
            <div className="form-group">
              <label htmlFor="key-title">Title</label>
              <input id="key-title" className="input" required value={title} placeholder="e.g. work laptop"
                onChange={(e) => setTitle(e.target.value)} />
            </div>
            <div className="form-group">
              <label htmlFor="key-body">Key</label>
              <textarea id="key-body" className="input textarea mono" rows={5} required value={key}
                placeholder="Begins with 'ssh-ed25519', 'ssh-rsa', 'ecdsa-sha2-nistp256', …"
                onChange={(e) => setKey(e.target.value)} />
            </div>
            <button type="submit" className="btn btn-primary" disabled={busy || !title.trim() || !key.trim()}>
              {busy ? "Adding…" : "Add SSH key"}
            </button>
          </form>
        </section>
        <section className="box">
          <div className="box-header"><h2 className="h5">Generating a key</h2></div>
          <div className="box-body setup">
            <CodeLine text={`ssh-keygen -t ed25519 -C "${user.email || user.login}"`} />
            <p className="small muted">Then paste the contents of <code>~/.ssh/id_ed25519.pub</code> above.</p>
          </div>
        </section>
      </div>
    </div>
  );
}

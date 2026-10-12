import { useState, type FormEvent } from "react";
import { Navigate } from "react-router-dom";
import { api, errorMessage, type Token } from "../api";
import { useAuth } from "../auth";
import { Async, Blankslate, CodeLine, ErrorBox, Flash, RelTime, Spinner } from "../components/common";
import { KeyIcon } from "../components/icons";
import { useAsync, useTitle } from "../hooks";
import { host, origin } from "../util";

export function TokensPage() {
  const { user, loading } = useAuth();
  useTitle("Personal access tokens");
  const st = useAsync(() => (user ? api.listTokens() : Promise.resolve([] as Token[])), [user?.login]);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const [created, setCreated] = useState<Token | null>(null);
  const [revokeErr, setRevokeErr] = useState("");
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Fsettings%2Ftokens" replace />;
  const h = host();
  const isHttps = location.protocol === "https:";

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const t = await api.createToken(name.trim());
      setCreated(t);
      setName("");
      st.reload();
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };
  const revoke = async (t: Token) => {
    if (!confirm(`Revoke token “${t.name}”? Any scripts or applications using it will stop working.`)) return;
    setRevokeErr("");
    try {
      await api.deleteToken(t.id);
      if (created?.id === t.id) setCreated(null);
      st.setData((st.data || []).filter((x) => x.id !== t.id));
    } catch (e) {
      setRevokeErr(errorMessage(e));
    }
  };

  return (
    <div className="container narrow">
      <h1 className="h2">Personal access tokens</h1>
      <p className="muted">
        Tokens work like passwords for <code>git</code> over HTTP and for the API (<code>Authorization: token …</code>), and are what the
        <code> gh</code> CLI uses.
      </p>

      <section className="box">
        <div className="box-header"><h2 className="h5">Generate new token</h2></div>
        <form className="box-body" onSubmit={submit}>
          {error !== undefined && <ErrorBox error={error} />}
          <div className="form-group">
            <label htmlFor="token-name">Note</label>
            <input id="token-name" className="input" required value={name} placeholder="e.g. laptop gh CLI"
              onChange={(e) => setName(e.target.value)} aria-describedby="token-name-hint" />
            <p id="token-name-hint" className="small muted">What's this token for?</p>
          </div>
          <button type="submit" className="btn btn-primary" disabled={busy || !name.trim()}>
            {busy ? "Generating…" : "Generate token"}
          </button>
        </form>
      </section>

      {created?.token && (
        <Flash kind="success">
          <div className="token-created">
            <p><strong>Make sure to copy your personal access token now.</strong> You won't be able to see it again!</p>
            <CodeLine text={created.token} label="New token" />
          </div>
        </Flash>
      )}

      <section className="box">
        <div className="box-header"><h2 className="h5">Your tokens</h2></div>
        {revokeErr && <ErrorBox error={new Error(revokeErr)} />}
        <Async state={st}>
          {(tokens) =>
            tokens.length === 0 ? (
              <Blankslate title="No tokens yet" icon={<KeyIcon size={24} />} />
            ) : (
              <ul className="list">
                {tokens.map((t) => (
                  <li key={t.id} className="list-row token-row">
                    <KeyIcon />
                    <div className="grow">
                      <strong>{t.name}</strong>
                      {t.token_last_eight && <span className="mono muted small"> …{t.token_last_eight}</span>}
                      <div className="muted small">
                        {t.created_at && <>Created <RelTime iso={t.created_at} />. </>}
                        {t.last_used_at ? <>Last used <RelTime iso={t.last_used_at} />.</> : "Never used."}
                      </div>
                    </div>
                    <button type="button" className="btn btn-sm btn-danger" onClick={() => revoke(t)}>Revoke</button>
                  </li>
                ))}
              </ul>
            )
          }
        </Async>
      </section>

      <section className="box">
        <div className="box-header"><h2 className="h5">Set up the gh CLI and git</h2></div>
        <div className="box-body setup">
          <p>Authenticate <code>gh</code> against this forge (paste the token on stdin):</p>
          <CodeLine text={`gh auth login --hostname ${h} --with-token < token.txt`} />
          <p className="small muted">or interactively: <code>echo "$TOKEN" | gh auth login --hostname {h} --with-token</code>.
            {!isHttps && <> gh talks to enterprise hosts over HTTPS by default, so put this server behind TLS for gh.</>}</p>
          <p>Then use gh as usual, e.g.:</p>
          <CodeLine text={`gh repo clone ${h}/${user.login}/REPO`} />
          <CodeLine text={`GH_HOST=${h} gh issue list -R ${user.login}/REPO`} />
          <p>Clone over HTTP with git (username: <code>{user.login}</code>, password: your token):</p>
          <CodeLine text={`git clone ${origin()}/${user.login}/REPO.git`} />
          <p className="small muted">Tip: cache the credential with <code>git config --global credential.helper store</code>, or let
            <code> gh auth setup-git --hostname {h}</code> configure git for you.</p>
        </div>
      </section>
    </div>
  );
}

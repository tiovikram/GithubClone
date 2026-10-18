// /organizations/new: "Set up your organization".
import { useState, type FormEvent } from "react";
import { Navigate, useNavigate } from "react-router-dom";
import { useAuth } from "../../auth";
import { ErrorBox, Spinner } from "../../components/common";
import { useTitle } from "../../hooks";
import { orgApi } from "./api";

export function NewOrgPage() {
  const { user, loading } = useAuth();
  const nav = useNavigate();
  const [login, setLogin] = useState("");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  useTitle("Set up your organization");
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Forganizations%2Fnew" replace />;
  const valid = /^[A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38}$/.test(login);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const org = await orgApi.create(login, name || undefined);
      if (email) await orgApi.update(org.login, { email, billing_email: email });
      nav(`/${org.login}`);
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  return (
    <div className="container narrow">
      <h1 className="h2">Set up your organization</h1>
      <p className="muted">Organizations are shared accounts where teams can collaborate across many projects at once.
        You will be the first owner of the new organization.</p>
      <form className="new-repo-form" onSubmit={submit}>
        {error !== undefined && <ErrorBox error={error} />}
        <div className="form-group">
          <label htmlFor="org-login">Organization name <span className="c-danger" aria-hidden="true">*</span></label>
          <input id="org-login" className="input" required autoFocus value={login} onChange={(e) => setLogin(e.target.value)}
            aria-invalid={login !== "" && !valid} aria-describedby="org-login-hint" />
          <p id="org-login-hint" className="small muted">
            {login && !valid ? "Use letters, digits and single hyphens (not at the start or end), up to 39 characters." :
              <>This will be the name of your account on forge. Your URL will be: <strong>{location.origin}/{login || "<name>"}</strong></>}
          </p>
        </div>
        <div className="form-group">
          <label htmlFor="org-display">Display name <span className="muted">(optional)</span></label>
          <input id="org-display" className="input" value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="org-email">Contact email <span className="muted">(optional)</span></label>
          <input id="org-email" type="email" className="input" value={email} onChange={(e) => setEmail(e.target.value)} />
        </div>
        <div className="form-actions">
          <button type="submit" className="btn btn-primary" disabled={busy || !valid}>{busy ? "Creating…" : "Create organization"}</button>
        </div>
      </form>
    </div>
  );
}

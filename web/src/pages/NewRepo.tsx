import { useState, type FormEvent } from "react";
import { Navigate, useNavigate } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../auth";
import { Avatar, ErrorBox, Spinner } from "../components/common";
import { useTitle } from "../hooks";

export function NewRepoPage() {
  const { user, loading } = useAuth();
  const nav = useNavigate();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [priv, setPriv] = useState(false);
  const [init, setInit] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  useTitle("New repository");
  if (loading) return <Spinner />;
  if (!user) return <Navigate to="/login?return_to=%2Fnew" replace />;
  const valid = /^[A-Za-z0-9._-]+$/.test(name) && name !== "." && name !== "..";
  const normalized = name.replace(/[^A-Za-z0-9._-]+/g, "-");
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const repo = await api.createRepo({ name, description: description || undefined, private: priv, auto_init: init });
      nav(`/${repo.full_name}`);
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  return (
    <div className="container narrow">
      <h1 className="h2">Create a new repository</h1>
      <p className="muted">A repository contains all project files, including the revision history.</p>
      <form onSubmit={submit} className="new-repo-form">
        {error !== undefined && <ErrorBox error={error} />}
        <div className="form-row">
          <div className="form-group">
            <span className="label-text">Owner</span>
            <div className="owner-pill"><Avatar user={user} size={20} /> {user.login}</div>
          </div>
          <span className="slash" aria-hidden="true">/</span>
          <div className="form-group grow">
            <label htmlFor="repo-name">Repository name <span className="c-danger" aria-hidden="true">*</span></label>
            <input id="repo-name" className="input" required autoFocus value={name} onChange={(e) => setName(e.target.value)}
              aria-invalid={name !== "" && !valid} aria-describedby="repo-name-hint" />
          </div>
        </div>
        <p id="repo-name-hint" className="small muted">
          {name && !valid ? <>Your new repository will be created as <strong>{normalized}</strong>. Use letters, digits, “.”, “-”, “_”.</> :
            "Great repository names are short and memorable."}
        </p>
        <div className="form-group">
          <label htmlFor="repo-desc">Description <span className="muted">(optional)</span></label>
          <input id="repo-desc" className="input" value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>
        <fieldset className="form-group radio-group">
          <legend>Visibility</legend>
          <label className="radio-row">
            <input type="radio" name="vis" checked={!priv} onChange={() => setPriv(false)} />
            <span><strong>Public</strong><br /><span className="muted small">Anyone on the internet can see this repository.</span></span>
          </label>
          <label className="radio-row">
            <input type="radio" name="vis" checked={priv} onChange={() => setPriv(true)} />
            <span><strong>Private</strong><br /><span className="muted small">You choose who can see and commit to this repository.</span></span>
          </label>
        </fieldset>
        <fieldset className="form-group">
          <legend>Initialize this repository with:</legend>
          <label className="check-row">
            <input type="checkbox" checked={init} onChange={(e) => setInit(e.target.checked)} />
            <span>Add a README file<br /><span className="muted small">This is where you can write a long description for your project.</span></span>
          </label>
        </fieldset>
        <div className="form-actions">
          <button type="submit" className="btn btn-primary" disabled={busy || !name}>
            {busy ? "Creating…" : "Create repository"}
          </button>
        </div>
      </form>
    </div>
  );
}

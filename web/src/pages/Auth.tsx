import { useState, type FormEvent } from "react";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../auth";
import { ErrorBox } from "../components/common";
import { LogoIcon } from "../components/icons";
import { useTitle } from "../hooks";

function safeReturn(to: string | null): string {
  return to && to.startsWith("/") && !to.startsWith("//") ? to : "/";
}

export function LoginPage() {
  const { user, setUser } = useAuth();
  const [sp] = useSearchParams();
  const nav = useNavigate();
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  useTitle("Sign in");
  const ret = safeReturn(sp.get("return_to"));
  if (user) return <Navigate to={ret} replace />;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      setUser(await api.login({ login: login.trim(), password }));
      nav(ret, { replace: true });
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  return (
    <div className="auth-page">
      <LogoIcon size={48} />
      <h1 className="h2">Sign in to forge</h1>
      {error !== undefined && <ErrorBox error={error} />}
      <form className="box auth-form" onSubmit={submit}>
        <div className="form-group">
          <label htmlFor="login">Username or email address</label>
          <input id="login" className="input" autoComplete="username" autoCapitalize="off" autoCorrect="off"
            required autoFocus value={login} onChange={(e) => setLogin(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="password">Password</label>
          <input id="password" type="password" className="input" autoComplete="current-password" required
            value={password} onChange={(e) => setPassword(e.target.value)} />
        </div>
        <button type="submit" className="btn btn-primary btn-block" disabled={busy}>
          {busy ? "Signing in…" : "Sign in"}
        </button>
      </form>
      <div className="box auth-alt">
        New to forge? <Link to={`/signup${ret !== "/" ? `?return_to=${encodeURIComponent(ret)}` : ""}`}>Create an account</Link>.
      </div>
    </div>
  );
}

export function SignupPage() {
  const { user, setUser } = useAuth();
  const [sp] = useSearchParams();
  const nav = useNavigate();
  const [login, setLogin] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  useTitle("Sign up");
  const ret = safeReturn(sp.get("return_to"));
  if (user) return <Navigate to={ret} replace />;
  const loginOk = /^[a-zA-Z0-9](?:[a-zA-Z0-9]|-(?=[a-zA-Z0-9])){0,38}$/.test(login);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const u = await api.signup({ login: login.trim(), email: email.trim(), password });
      setUser(u);
      nav(ret, { replace: true });
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  return (
    <div className="auth-page">
      <LogoIcon size={48} />
      <h1 className="h2">Create your account</h1>
      {error !== undefined && <ErrorBox error={error} />}
      <form className="box auth-form" onSubmit={submit}>
        <div className="form-group">
          <label htmlFor="su-login">Username</label>
          <input id="su-login" className="input" autoComplete="username" autoCapitalize="off" required autoFocus
            value={login} onChange={(e) => setLogin(e.target.value)} aria-describedby="su-login-hint"
            aria-invalid={login !== "" && !loginOk} />
          <p id="su-login-hint" className={`small ${login && !loginOk ? "c-danger" : "muted"}`}>
            Alphanumeric characters or single hyphens; cannot begin or end with a hyphen.
          </p>
        </div>
        <div className="form-group">
          <label htmlFor="su-email">Email</label>
          <input id="su-email" type="email" className="input" autoComplete="email" required
            value={email} onChange={(e) => setEmail(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="su-password">Password</label>
          <input id="su-password" type="password" className="input" autoComplete="new-password" required minLength={8}
            value={password} onChange={(e) => setPassword(e.target.value)} aria-describedby="su-pw-hint" />
          <p id="su-pw-hint" className="small muted">At least 8 characters.</p>
        </div>
        <button type="submit" className="btn btn-primary btn-block" disabled={busy || !loginOk}>
          {busy ? "Creating account…" : "Create account"}
        </button>
      </form>
      <div className="box auth-alt">
        Already have an account? <Link to="/login">Sign in</Link>.
      </div>
    </div>
  );
}

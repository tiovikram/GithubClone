import { useEffect, useRef, useState } from "react";
import { Link, NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import { useAuth } from "../auth";
import { Avatar } from "./common";
import { ChevronDownIcon, LogoIcon, PlusIcon } from "./icons";

function UserMenu() {
  const { user, logout } = useAuth();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const nav = useNavigate();
  const loc = useLocation();
  useEffect(() => setOpen(false), [loc.pathname]);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof KeyboardEvent ? e.key === "Escape" : !ref.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", close);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", close);
    };
  }, [open]);
  if (!user) return null;
  return (
    <div className="menu-wrap" ref={ref}>
      <button type="button" className="header-btn" aria-haspopup="menu" aria-expanded={open}
        aria-label="Open user menu" onClick={() => setOpen(!open)}>
        <Avatar user={user} size={24} />
        <ChevronDownIcon size={12} />
      </button>
      {open && (
        <div className="menu" role="menu">
          <div className="menu-header">
            Signed in as <strong>{user.login}</strong>
          </div>
          <Link role="menuitem" className="menu-item" to={`/${user.login}`}>Your profile</Link>
          <Link role="menuitem" className="menu-item" to={`/${user.login}?tab=repositories`}>Your repositories</Link>
          <Link role="menuitem" className="menu-item" to="/new">New repository</Link>
          <div className="menu-divider" />
          <Link role="menuitem" className="menu-item" to="/settings/tokens">Access tokens</Link>
          <div className="menu-divider" />
          <button role="menuitem" type="button" className="menu-item"
            onClick={async () => {
              await logout();
              nav("/");
            }}>
            Sign out
          </button>
        </div>
      )}
    </div>
  );
}

export function Layout() {
  const { user, loading } = useAuth();
  const loc = useLocation();
  return (
    <div className="app">
      <a className="skip-link" href="#main">Skip to content</a>
      <header className="header">
        <Link to="/" className="header-logo" aria-label="forge home">
          <LogoIcon size={30} />
          <span className="header-title">forge</span>
        </Link>
        <nav className="header-nav" aria-label="Global">
          {user && <NavLink to="/" end>Dashboard</NavLink>}
        </nav>
        <div className="header-right">
          {loading ? null : user ? (
            <>
              <Link to="/new" className="header-btn" aria-label="Create new repository" title="New repository">
                <PlusIcon />
              </Link>
              <UserMenu />
            </>
          ) : (
            <>
              {loc.pathname !== "/login" && (
                <Link to={`/login?return_to=${encodeURIComponent(loc.pathname + loc.search)}`} className="header-link">
                  Sign in
                </Link>
              )}
              {loc.pathname !== "/signup" && (
                <Link to="/signup" className="btn btn-sm btn-outline-light">Sign up</Link>
              )}
            </>
          )}
        </div>
      </header>
      <main id="main" className="main">
        <Outlet />
      </main>
      <footer className="footer muted">
        <span>forge — a GitHub-compatible code forge</span>
        <a href="/api/v3" rel="nofollow">API</a>
      </footer>
    </div>
  );
}

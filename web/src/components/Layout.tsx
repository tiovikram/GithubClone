import { useEffect, useRef, useState } from "react";
import { Link, NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import { useAuth } from "../auth";
import { headerNav, Slot, userMenu } from "../areas";
import { AnnouncementBanner } from "./Announcement";
import { Avatar } from "./common";
import { ChevronDownIcon, LogoIcon, MoonIcon, PlusIcon, SearchIcon, SunIcon } from "./icons";

type Theme = "auto" | "light" | "dark";

function readTheme(): Theme {
  try {
    const t = localStorage.getItem("forge-theme");
    return t === "light" || t === "dark" ? t : "auto";
  } catch {
    return "auto";
  }
}

export function applyTheme(t: Theme) {
  const root = document.documentElement;
  if (t === "auto") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", t);
}

/** Cycles auto → light → dark; "auto" follows prefers-color-scheme. */
function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(readTheme);
  useEffect(() => {
    applyTheme(theme);
    try {
      if (theme === "auto") localStorage.removeItem("forge-theme");
      else localStorage.setItem("forge-theme", theme);
    } catch {
      /* storage unavailable */
    }
  }, [theme]);
  const next: Theme = theme === "auto" ? "light" : theme === "light" ? "dark" : "auto";
  return (
    <button type="button" className="header-btn" onClick={() => setTheme(next)} data-testid="theme-toggle"
      aria-label={`Theme: ${theme}. Switch to ${next}.`} title={`Theme: ${theme}`}>
      {theme === "dark" ? <MoonIcon /> : theme === "light" ? <SunIcon /> : <><SunIcon size={12} /><MoonIcon size={12} /></>}
    </button>
  );
}

function HeaderSearch() {
  const nav = useNavigate();
  const loc = useLocation();
  const [q, setQ] = useState("");
  // Inside a repo, scope the search to it like github.com does.
  const m = loc.pathname.match(/^\/([^/]+)\/([^/]+)/);
  const reserved = ["settings", "search", "login", "signup", "new"];
  const scope = m && !reserved.includes(m[1]) ? `repo:${m[1]}/${m[2]} ` : "";
  return (
    <form className="header-search" role="search" onSubmit={(e) => {
      e.preventDefault();
      if (!q.trim()) return;
      nav(`/search?q=${encodeURIComponent(scope + q.trim())}&type=${scope ? "code" : "repositories"}`);
      setQ("");
    }}>
      <SearchIcon />
      <label htmlFor="header-q" className="sr-only">Search</label>
      <input id="header-q" className="header-search-input" value={q} onChange={(e) => setQ(e.target.value)}
        placeholder={scope ? "Search this repository" : "Search forge"} />
    </form>
  );
}

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
          <Link role="menuitem" className="menu-item" to={`/${user.login}?tab=stars`}>Your stars</Link>
          <div className="menu-divider" />
          {userMenu.filter((l) => !l.show || l.show(user)).map((l) => (
            <Link key={l.key} role="menuitem" className="menu-item" to={typeof l.to === "string" ? l.to : l.to(user)}>{l.label}</Link>
          ))}
          <Link role="menuitem" className="menu-item" to="/settings">Settings</Link>
          <Link role="menuitem" className="menu-item" to="/settings/tokens">Access tokens</Link>
          <Link role="menuitem" className="menu-item" to="/settings/keys">SSH keys</Link>
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
          {user && headerNav.filter((l) => !l.show || l.show(user)).map((l) => (
            <NavLink key={l.key} to={typeof l.to === "string" ? l.to : l.to(user)}>{l.label}</NavLink>
          ))}
        </nav>
        <HeaderSearch />
        <div className="header-right">
          {!loading && <Slot name="header.right" user={user} />}
          <ThemeToggle />
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
      <AnnouncementBanner />
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

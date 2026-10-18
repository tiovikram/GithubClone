// Site administration (/site-admin/*) end to end. Needs --login to be a site admin (the first
// account on a fresh database). Covers every admin page and action, the user-facing effects
// (suspension, announcement banner, maintenance page, pre-receive hook rejecting a push) and the
// org/repo "Pre-receive hooks" settings sections. Maintenance mode is always turned off again.
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export async function run(t) {
  const { step, goto, waitText, waitBody, click, fill, clickText, api, shot, assert, log, RUN, LOGIN, LOGIN2, BASE, WORK } = t;
  const page = () => t.page;
  const me = () => t.me;
  const sel = (id) => `[data-testid="${id}"]`;
  const ORG = `sa-org-${RUN}`;
  const ORG2 = `sa-org2-${RUN}`;
  const NEWU = `sa-new-${RUN}`;
  const RENU = `sa-ren-${RUN}`;
  const SCRIPTS = `sa-hooks-${RUN}`;
  const TARGET = `sa-target-${RUN}`;
  const DOOMED = `sa-doomed-${RUN}`;
  const HOOKNAME = `e2e-reject-${RUN}`;
  let ent = "forge";

  /** Wait until the element matching `selector` contains `text`. */
  const waitIn = (selector, text, p = page(), timeout = 30000) =>
    p.waitForFunction((s, x) => [...document.querySelectorAll(s)].some((e) => e.innerText.includes(x)), { timeout }, selector, text);
  const waitGone = (selector, p = page(), timeout = 30000) =>
    p.waitForFunction((s) => !document.querySelector(s), { timeout }, selector);
  const nav = async (path, p = page()) => {
    await goto(path, p);
    await p.waitForSelector(sel("site-admin"), { timeout: 30000 }).catch(() => undefined);
  };
  /** Click the button containing `text` inside the element matching `scope`. */
  const clickIn = (scope, text, tag = "button") => page().locator(`${scope} ${tag}::-p-text(${text})`).click();
  const maintenanceOff = () => api("/manage/v1/maintenance", { method: "POST", body: { enabled: false } });

  // ---------- preconditions ----------
  await step("admin: signed-in user is a site admin", async () => {
    const r = await api("/user");
    assert(r.status === 200, `GET /user ${r.status}`);
    if (!r.json.site_admin) {
      throw new Error(`${LOGIN} is not a site admin. Run the admin e2e against a fresh database (the first account becomes site admin) or pass the first login with --login.`);
    }
    // A failed earlier run may have left the second user suspended or promoted, or maintenance on.
    await maintenanceOff();
    const u2 = await api(`/users/${LOGIN2}`);
    if (u2.status === 200) {
      await api(`/users/${LOGIN2}/suspended`, { method: "DELETE", body: {} });
      if (u2.json.site_admin) await api(`/users/${LOGIN2}/site_admin`, { method: "DELETE" });
    }
    const info = await t.pageFetch("/api/forge/admin/info");
    ent = JSON.parse(info.text).enterprise_slug;
  });

  const p2 = await t.page2();

  await step("admin: user menu links to the site admin shell", async () => {
    await goto("/");
    await click('button[aria-label="Open user menu"]');
    await clickText("Site admin", "a");
    await page().waitForSelector(sel("sa-overview"));
    await page().waitForSelector('nav[aria-label="Site admin"]');
    for (const k of ["overview", "users", "organizations", "repositories", "announcement", "maintenance", "settings", "license", "hooks", "pre-receive-hooks", "pre-receive-environments", "ldap", "scim", "audit-log", "live-migrations", "enterprise-teams"]) {
      assert(await page().$(sel(`settings-nav-${k}`)), `sidebar link ${k}`);
    }
    await waitIn(sel("sa-stats-users"), "Site admins");
    await shot("admin-01-overview");
  });

  await step("admin: hidden from non-admins (menu entry and 404)", async () => {
    await goto("/", p2);
    await click('button[aria-label="Open user menu"]', p2);
    const items = await p2.$$eval('[role="menu"] a', (as) => as.map((a) => a.textContent));
    assert(!items.includes("Site admin"), `non-admin menu: ${items.join(", ")}`);
    await goto("/site-admin/users", p2);
    await waitText("This is not the page you are looking for", p2);
    const r = await api("/admin/hooks", {}, p2);
    assert(r.status === 403, `non-admin GET /admin/hooks ${r.status}`);
  });

  // ---------- users ----------
  await step("admin: search users and open a user", async () => {
    await nav("/site-admin/users");
    await fill("#sa-user-q", LOGIN2);
    await clickIn(".sa-filter", "Search");
    await page().waitForSelector(sel(`sa-user-${LOGIN2}`));
    await click(`${sel(`sa-user-${LOGIN2}`)} a`);
    await page().waitForSelector(sel("sa-user-detail"));
    await waitIn(sel("sa-user-detail"), "Repositories");
    await shot("admin-02-user");
  });

  await step("admin: suspend user with reason (cannot sign in, API refused)", async () => {
    await fill("#sa-suspend-reason", `e2e reason ${RUN}`);
    await click(sel("sa-suspend"));
    await waitIn(sel("sa-suspension"), `e2e reason ${RUN}`);
    const who = await api("/user", {}, p2);
    assert(who.status === 401 || who.status === 403, `suspended user's session: GET /user ${who.status}`);
    await goto("/login", p2);
    await fill("#login", LOGIN2, p2);
    await fill("#password", t.PASSWORD, p2);
    await clickText("Sign in", "button", p2);
    await p2.waitForSelector('[role="alert"]', { timeout: 15000 });
    assert(new URL(p2.url()).pathname === "/login", `suspended user signed in: ${p2.url()}`);
    await shot("admin-03-suspended-login", p2);
    const list = await t.pageFetch(`/api/forge/admin/users?filter=suspended&q=${LOGIN2}`);
    assert(JSON.parse(list.text).total_count === 1, "listed as suspended");
  });

  await step("admin: unsuspend user (can sign in again)", async () => {
    await click(sel("sa-unsuspend"));
    await waitIn(sel("sa-suspension"), "Active");
    await t.signUpOrIn(p2, LOGIN2, t.PASSWORD);
    const who = await api("/user", {}, p2);
    assert(who.status === 200 && who.json.login === LOGIN2, `GET /user after unsuspend ${who.status}`);
  });

  await step("admin: promote and demote site admin", async () => {
    await click(sel("sa-promote"));
    await page().waitForSelector(sel("sa-demote"));
    let u = await api(`/users/${LOGIN2}`);
    assert(u.json.site_admin === true, "promoted");
    await goto("/site-admin", p2);
    await p2.waitForSelector(sel("sa-overview"));
    await click(sel("sa-demote"));
    await page().waitForSelector(sel("sa-promote"));
    u = await api(`/users/${LOGIN2}`);
    assert(u.json.site_admin === false, "demoted");
    await goto("/", p2);
    await goto("/site-admin", p2);
    await waitText("This is not the page you are looking for", p2);
  });

  await step("admin: LDAP mapping and sync from the user page", async () => {
    await fill("#sa-ldap-dn", `uid=${LOGIN2},ou=people,dc=example,dc=com`);
    await clickIn(sel("sa-user-detail"), "Update mapping");
    await waitText("LDAP mapping updated.");
    await waitIn(sel("sa-user-detail"), `uid=${LOGIN2},ou=people`);
    await clickIn(sel("sa-user-detail"), "Sync now");
    await waitText("LDAP sync queued.");
  });

  await step("admin: create user with password, rename, impersonate, delete", async () => {
    await nav("/site-admin/users");
    await click(sel("sa-new-user"));
    await fill("#sa-cu-login", NEWU);
    await fill("#sa-cu-email", `${NEWU}@example.com`);
    await fill("#sa-cu-pass", "e2e-new-pass-1");
    await clickText("Create user", "button[type=submit]");
    await page().waitForSelector(sel("sa-user-detail"));
    await waitIn(sel("sa-user-detail"), NEWU);
    await fill("#sa-rename", RENU);
    await clickIn(sel("sa-user-detail"), "Rename", "button[type=submit]");
    await page().waitForFunction((n) => location.pathname.endsWith(`/users/${n}`), {}, RENU);
    await waitIn(sel("sa-user-detail"), RENU);
    // impersonation token works as the user
    await click(sel("sa-impersonate"));
    await page().waitForSelector(sel("sa-impersonation-token"));
    const tok = await page().$eval(`${sel("sa-impersonation-token")} input`, (e) => e.value);
    const r = await fetch(`${BASE}/api/v3/user`, { headers: { Authorization: `token ${tok}`, Accept: "application/vnd.github+json" } });
    const body = await r.json();
    assert(r.status === 200 && body.login === RENU, `impersonation token: ${r.status} ${body.login}`);
    await waitIn(sel("sa-user-tokens"), "impersonation");
    await shot("admin-04-impersonation");
    await clickIn(sel("sa-user-detail"), "Revoke all");
    await waitText("Impersonation tokens revoked.");
    const r2 = await fetch(`${BASE}/api/v3/user`, { headers: { Authorization: `token ${tok}` } });
    assert(r2.status === 401, `revoked impersonation token: ${r2.status}`);
    // reset password, then sign-in check through the API
    await fill("#sa-password", "e2e-reset-pass-2");
    await clickIn(sel("sa-user-detail"), "Set password");
    await waitText("Password updated");
    const s = await fetch(`${BASE}/api/forge/session`, { method: "POST", headers: { "Content-Type": "application/json", "X-Forge-CSRF": "1" }, body: JSON.stringify({ login: RENU, password: "e2e-reset-pass-2" }) });
    assert(s.status === 200, `sign in with reset password: ${s.status}`);
    await fill("#sa-del-user", RENU);
    await clickText("Delete user", "button[type=submit]");
    await page().waitForSelector(sel("sa-users"));
    const gone = await api(`/users/${RENU}`);
    assert(gone.status === 404, `deleted user: ${gone.status}`);
  });

  // ---------- organizations and repositories ----------
  await step("admin: create organization with owner and rename it", async () => {
    await nav("/site-admin/organizations");
    await click(sel("sa-new-org"));
    await fill("#sa-co-login", ORG);
    await fill("#sa-co-admin", me());
    await fill("#sa-co-name", "E2E Org");
    await clickText("Create organization", "button[type=submit]");
    await page().waitForSelector(sel(`sa-org-${ORG}`));
    await clickIn(sel(`sa-org-${ORG}`), "Rename…");
    await fill(`${sel(`sa-org-${ORG}`)} input`, ORG2);
    await clickIn(sel(`sa-org-${ORG}`), "Rename", "button[type=submit]");
    await page().waitForSelector(sel(`sa-org-${ORG2}`));
    await shot("admin-05-orgs");
    const o = await api(`/orgs/${ORG2}`);
    assert(o.status === 200, `renamed org: ${o.status}`);
  });

  await step("admin: repositories search, archive, unarchive, delete", async () => {
    await t.newRepo(DOOMED, { private: true });
    await nav("/site-admin/repositories");
    await fill("#sa-repo-q", DOOMED);
    await clickIn(".sa-filter", "Search");
    const row = sel(`sa-repo-${me()}/${DOOMED}`);
    await page().waitForSelector(row);
    await waitIn(row, "private");
    await clickIn(row, "Archive");
    await waitIn(row, "archived");
    await clickIn(row, "Unarchive");
    await page().waitForFunction((s) => !document.querySelector(s)?.innerText.includes("archived"), {}, row);
    await clickIn(row, "Delete…");
    await fill(`${row} input`, `${me()}/${DOOMED}`);
    await clickIn(row, "Delete", "button[type=submit]");
    await waitGone(row);
    const r = await api(`/repos/${me()}/${DOOMED}`);
    assert(r.status === 404, `deleted repo: ${r.status}`);
  });

  // ---------- announcement ----------
  await step("admin: announcement shows for other users, then clears", async () => {
    await nav("/site-admin/announcement");
    await fill("#sa-ann-text", `E2E announcement ${RUN} **bold**`);
    await page().$eval("#sa-ann-dismiss", (e) => { if (!e.checked) e.click(); });
    await click(sel("sa-ann-save"));
    await waitText("Announcement saved.");
    await goto("/", p2);
    await waitIn(sel("announcement-banner"), `E2E announcement ${RUN}`, p2);
    assert(await p2.$('button[aria-label="Dismiss announcement"]'), "dismissible");
    await shot("admin-06-announcement", p2);
    await click(sel("sa-ann-clear"));
    await waitText("Announcement removed.");
    await goto("/", p2);
    await waitText("Welcome", p2);
    assert(!(await p2.$(sel("announcement-banner"))), "banner cleared");
  });

  // ---------- maintenance ----------
  await step("admin: maintenance mode on shows the maintenance page to users, then off", async () => {
    try {
      await nav("/site-admin/maintenance");
      await fill("#sa-maint-msg", `E2E maintenance ${RUN}`);
      await click(sel("sa-maint-on"));
      await waitText("Maintenance mode is on.");
      await waitIn(sel("sa-maint-status"), "on");
      await goto("/", p2);
      await p2.waitForSelector(sel("maintenance-page"));
      await waitIn(sel("maintenance-page"), `E2E maintenance ${RUN}`, p2);
      await shot("admin-07-maintenance-page", p2);
      const r = await api("/user", {}, p2);
      assert(r.status === 503, `API during maintenance: ${r.status}`);
      // site admins keep access and see a reminder
      await goto("/");
      await page().waitForSelector(sel("maintenance-admin-bar"));
      const me1 = await api("/user");
      assert(me1.status === 200, `admin during maintenance: ${me1.status}`);
      await nav("/site-admin/maintenance");
      await click(sel("sa-maint-off"));
      await waitText("Maintenance mode is off.");
      await goto("/", p2);
      await waitText("Welcome", p2);
      assert(!(await p2.$(sel("maintenance-page"))), "maintenance page gone");
    } finally {
      await maintenanceOff();
    }
  });

  await step("admin: schedule maintenance (users see a notice), then cancel", async () => {
    try {
      await nav("/site-admin/maintenance");
      const future = new Date(Date.now() + 2 * 86400000);
      const pad = (n) => String(n).padStart(2, "0");
      const v = `${future.getFullYear()}-${pad(future.getMonth() + 1)}-${pad(future.getDate())}T10:00`;
      await page().$eval("#sa-maint-when", (e, val) => {
        const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
        set.call(e, val);
        e.dispatchEvent(new Event("input", { bubbles: true }));
      }, v);
      await click(sel("sa-maint-schedule"));
      await waitText("Maintenance scheduled.");
      await waitIn(sel("sa-maint-status"), "scheduled");
      await goto("/", p2);
      await p2.waitForSelector(sel("maintenance-scheduled-bar"));
      assert(!(await p2.$(sel("maintenance-page"))), "scheduled is not active");
      await shot("admin-08-maintenance-scheduled");
      await click(sel("sa-maint-off"));
      await waitText("Maintenance mode is off.");
    } finally {
      await maintenanceOff();
    }
  });

  // ---------- management console ----------
  await step("admin: settings save and apply", async () => {
    await nav("/site-admin/settings");
    await page().waitForSelector("#sa-set-smtp-domain");
    await fill("#sa-set-smtp-domain", `e2e-${RUN}.example.com`);
    await click(sel("sa-settings-apply"));
    await waitText("Settings saved and applied.");
    const s = await api("/manage/v1/config/settings");
    assert(s.json.smtp.domain === `e2e-${RUN}.example.com`, `smtp.domain ${s.json.smtp?.domain}`);
    await waitIn(sel("sa-apply-status"), "successful");
    await shot("admin-09-settings");
  });

  await step("admin: nodes, license and SSH access pages", async () => {
    await nav("/site-admin/nodes");
    await waitIn(sel("sa-nodes"), "postgresql");
    await nav("/site-admin/license");
    await waitIn(sel("sa-license-seats"), "unlimited");
    await nav("/site-admin/ssh-access");
    const k = spawnSync("ssh-keygen", ["-t", "ed25519", "-N", "", "-q", "-C", `sa-${RUN}`, "-f", join(WORK, `sa-key-${RUN}`)]);
    assert(k.status === 0, "ssh-keygen");
    const pub = (await import("node:fs")).readFileSync(join(WORK, `sa-key-${RUN}.pub`), "utf8").trim();
    await fill("#sa-ssh-key", pub);
    await clickText("Add key", "button[type=submit]");
    await waitText("Key added.");
    await waitIn(sel("sa-ssh-access"), "SHA256:");
    await clickIn(sel("sa-ssh-access"), "Remove");
    await waitText("Key removed.");
  });

  // ---------- global webhooks ----------
  await step("admin: global webhook create, ping, edit, delete", async () => {
    await nav("/site-admin/hooks");
    await click(sel("sa-hook-new"));
    await fill("#sa-hook-url", `http://127.0.0.1:9/e2e-${RUN}`);
    await fill("#sa-hook-secret", "e2e-secret");
    await click(sel("sa-hook-save"));
    await page().waitForSelector(sel("sa-hooks"));
    await waitIn(sel("sa-hooks"), `e2e-${RUN}`);
    const hooks = (await api("/admin/hooks")).json;
    const h = hooks.find((x) => x.config.url.endsWith(`e2e-${RUN}`));
    assert(h && h.events.includes("user") && h.events.includes("organization"), "hook created with both events");
    await clickIn(sel(`sa-hook-${h.id}`), "Ping");
    await waitText("Ping sent");
    await clickIn(sel(`sa-hook-${h.id}`), "Edit", "a");
    await page().waitForSelector(sel("sa-hook-editor"));
    await page().$eval("#sa-hook-active", (e) => e.click());
    await click(sel("sa-hook-save"));
    await waitIn(sel(`sa-hook-${h.id}`), "inactive");
    await shot("admin-10-hooks");
    await clickIn(sel(`sa-hook-${h.id}`), "Delete");
    await waitGone(sel(`sa-hook-${h.id}`));
  });

  // ---------- pre-receive ----------
  const pushTarget = (branch) => {
    const dir = join(WORK, `sa-push-${branch}-${Date.now()}`);
    const url = new URL(`${BASE}/${me()}/${TARGET}.git`);
    url.username = me();
    url.password = t.token;
    t.git(WORK, "clone", "-q", url.toString(), dir);
    t.git(dir, "checkout", "-q", "-b", branch);
    writeFileSync(join(dir, `${branch}.txt`), `${branch}\n`);
    t.git(dir, "add", "-A");
    t.git(dir, "commit", "-q", "-m", `push ${branch}`);
    const r = spawnSync("git", ["push", "origin", branch], {
      cwd: dir, encoding: "utf8",
      env: { ...process.env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null", GIT_TERMINAL_PROMPT: "0" },
    });
    return { ok: r.status === 0, out: `${r.stdout}\n${r.stderr}` };
  };

  await step("admin: pre-receive environment add (download fails for an unreachable image), delete", async () => {
    await nav("/site-admin/pre-receive-environments");
    await waitIn(sel("sa-prenvs"), "default");
    await fill("#sa-env-name", `e2e-env-${RUN}`);
    await fill("#sa-env-url", `http://127.0.0.1:9/env-${RUN}.tar.gz`);
    await clickText("Add environment", "button[type=submit]");
    await waitText("Environment added");
    const envs = (await api("/admin/pre-receive-environments")).json;
    const env = envs.find((e) => e.name === `e2e-env-${RUN}`);
    assert(env, "environment created");
    await waitIn(sel(`sa-env-${env.id}`), "failed", page(), 45000);
    await shot("admin-11-environments");
    await clickIn(sel(`sa-env-${env.id}`), "Delete");
    await waitGone(sel(`sa-env-${env.id}`));
  });

  let hookId;
  await step("admin: pre-receive hook rejects a push", async () => {
    await t.newRepo(SCRIPTS);
    const script = "#!/bin/sh\necho \"E2E pre-receive says no\"\nexit 1\n";
    const put = await t.rest(`/repos/${me()}/${SCRIPTS}/contents/reject.sh`, {
      method: "PUT", body: { message: "hook script", content: Buffer.from(script).toString("base64") },
    });
    assert(put.status === 201, `script file: ${put.status} ${put.text}`);
    await t.newRepo(TARGET);
    await nav("/site-admin/pre-receive-hooks");
    await click(sel("sa-prehook-new"));
    await fill("#sa-ph-name", HOOKNAME);
    await fill("#sa-ph-repo", `${me()}/${SCRIPTS}`);
    await fill("#sa-ph-script", "reject.sh");
    await page().$eval('input[name="sa-ph-enf"][value="enabled"]', (e) => e.click());
    await page().$eval("#sa-ph-downstream", (e) => { if (!e.checked) e.click(); });
    await page().waitForFunction(() => document.querySelector("#sa-ph-env")?.value);
    await click(sel("sa-prehook-save"));
    await page().waitForSelector(sel(`sa-prehook-${HOOKNAME}`));
    await waitIn(sel(`sa-prehook-${HOOKNAME}`), "enabled");
    await shot("admin-12-prehooks");
    hookId = (await api("/admin/pre-receive-hooks")).json.find((h) => h.name === HOOKNAME).id;
    const r = pushTarget("rejected");
    assert(!r.ok, `push should be rejected:\n${r.out}`);
    assert(r.out.includes("E2E pre-receive says no"), `hook output shown:\n${r.out}`);
  });

  await step("admin: repository settings override pre-receive hook enforcement", async () => {
    await goto(`/${me()}/${TARGET}/settings/pre-receive-hooks`);
    await page().waitForSelector(sel("repo-prehooks"));
    const row = sel(`hooks-row-${HOOKNAME}`);
    await page().waitForSelector(row);
    await page().select(`${row} select`, "testing");
    await clickIn(row, "Save", "button[type=submit]");
    await waitText("Enforcement updated.");
    await waitIn(row, "repository level");
    await shot("admin-13-repo-prehooks");
    const r = pushTarget("testing");
    assert(r.ok, `push in testing mode should pass:\n${r.out}`);
    assert(r.out.includes("E2E pre-receive says no"), `testing output shown:\n${r.out}`);
    await clickIn(row, "Use inherited setting");
    await waitText("Using the inherited enforcement.");
    await waitIn(row, "enterprise level");
  });

  await step("admin: organization settings pre-receive hooks section", async () => {
    await goto(`/organizations/${ORG2}/settings/pre-receive-hooks`);
    const shell = await page().waitForSelector(".settings-nav", { timeout: 8000 }).catch(() => null);
    if (!shell) {
      log("    SKIP organization settings shell not rendered (orgs area not merged); org Pre-receive hooks section untested in the browser");
      const r = await api(`/orgs/${ORG2}/pre-receive-hooks/${hookId}`, { method: "PATCH", body: { enforcement: "testing" } });
      assert(r.status === 200 && r.json.enforcement === "testing", `org override via API: ${r.status}`);
      await api(`/orgs/${ORG2}/pre-receive-hooks/${hookId}`, { method: "DELETE" });
      return;
    }
    await page().waitForSelector(sel("org-prehooks"));
    const row = sel(`hooks-row-${HOOKNAME}`);
    await page().select(`${row} select`, "testing");
    await clickIn(row, "Save", "button[type=submit]");
    await waitText("Enforcement updated.");
    await waitIn(row, "organization level");
    await shot("admin-14-org-prehooks");
    await clickIn(row, "Use inherited setting");
    await waitIn(row, "enterprise level");
  });

  await step("admin: delete pre-receive hook (push accepted again)", async () => {
    await nav("/site-admin/pre-receive-hooks");
    await clickIn(sel(`sa-prehook-${HOOKNAME}`), "Delete");
    await waitGone(sel(`sa-prehook-${HOOKNAME}`));
    const r = pushTarget("accepted");
    assert(r.ok, `push after hook deletion:\n${r.out}`);
  });

  // ---------- identity ----------
  await step("admin: LDAP page maps and syncs a team", async () => {
    const team = await api(`/orgs/${ORG2}/teams`, { method: "POST", body: { name: `ldap-${RUN}` } });
    assert(team.status === 201, `team: ${team.status}`);
    await nav("/site-admin/ldap");
    await fill("#sa-ldap-org", ORG2);
    await fill("#sa-ldap-team", team.json.slug);
    await fill("#sa-ldap-tdn", `cn=ldap-${RUN},ou=groups,dc=example,dc=com`);
    await clickIn('form[aria-label="LDAP team mapping"]', "Update mapping");
    await waitText(`LDAP mapping of ${ORG2}/${team.json.slug} updated.`);
    await clickIn('form[aria-label="LDAP team mapping"]', "Sync team");
    await waitText("queued");
    await fill("#sa-ldap-user", LOGIN2);
    await fill("#sa-ldap-udn", `uid=${LOGIN2},dc=example,dc=com`);
    await clickIn('form[aria-label="LDAP user mapping"]', "Update mapping");
    await waitText(`LDAP mapping of ${LOGIN2} updated.`);
  });

  await step("admin: SCIM lists provisioned users and groups", async () => {
    const scimUser = `scim-${RUN}`;
    const headers = { "Content-Type": "application/scim+json", Accept: "application/scim+json" };
    const u = await t.rest(`/scim/v2/enterprises/${ent}/Users`, {
      method: "POST", headers,
      body: JSON.stringify({ schemas: ["urn:ietf:params:scim:schemas:core:2.0:User"], userName: scimUser, externalId: `ext-${RUN}`, active: true, displayName: "SCIM E2E", emails: [{ value: `${scimUser}@example.com`, primary: true, type: "work" }], roles: [] }),
    });
    assert(u.status === 201, `SCIM user: ${u.status} ${u.text}`);
    const g = await t.rest(`/scim/v2/enterprises/${ent}/Groups`, {
      method: "POST", headers,
      body: JSON.stringify({ schemas: ["urn:ietf:params:scim:schemas:core:2.0:Group"], displayName: `group-${RUN}`, externalId: `gext-${RUN}`, members: [{ value: u.json.id }] }),
    });
    assert(g.status === 201, `SCIM group: ${g.status} ${g.text}`);
    await nav("/site-admin/scim");
    await waitIn(sel("sa-scim"), scimUser);
    await waitIn(sel("sa-scim"), `group-${RUN}`);
    await clickText(scimUser, "a");
    await page().waitForSelector(sel("sa-scim-user"));
    await waitIn(sel("sa-scim-user"), `ext-${RUN}`);
    await nav("/site-admin/scim");
    await clickText(`group-${RUN}`, "a");
    await page().waitForSelector(sel("sa-scim-group"));
    await waitIn(sel("sa-scim-group"), scimUser);
  });

  // ---------- enterprise teams ----------
  await step("admin: enterprise team create, members, organization assignment, delete", async () => {
    await nav("/site-admin/enterprise-teams");
    await fill("#sa-et-name", `E2E Team ${RUN}`);
    await fill("#sa-et-desc", "created by the e2e");
    await clickText("Create team", "button[type=submit]");
    await page().waitForSelector(sel("sa-ent-team"));
    await fill("#sa-et-member", LOGIN2);
    await clickIn(sel("sa-ent-team-members"), "Add member", "button[type=submit]");
    await waitIn(sel("sa-ent-team-members"), LOGIN2);
    await fill("#sa-et-org", ORG2);
    await clickIn(sel("sa-ent-team-orgs"), "Assign", "button[type=submit]");
    await waitIn(sel("sa-ent-team-orgs"), ORG2);
    const m = await api(`/orgs/${ORG2}/members/${LOGIN2}`);
    assert(m.status === 204, `team assignment grants org membership: ${m.status}`);
    await shot("admin-15-enterprise-team");
    await clickIn(sel("sa-ent-team-orgs"), "Unassign");
    await waitIn(sel("sa-ent-team-orgs"), "Not assigned");
    await clickIn(sel("sa-ent-team-members"), "Remove");
    await waitIn(sel("sa-ent-team-members"), "No members");
    await clickIn(sel("sa-ent-team"), "Delete team");
    await page().waitForSelector(sel("sa-ent-teams"));
    await page().waitForFunction((n) => !document.body.innerText.includes(n), {}, `E2E Team ${RUN}`);
  });

  // ---------- live migrations ----------
  await step("admin: live migration create and start (recorded, fails to transfer)", async () => {
    await nav("/site-admin/live-migrations");
    await fill("#sa-mig-source_organization_login", me());
    await fill("#sa-mig-source_repository_name", TARGET);
    await fill("#sa-mig-target_organization_login", "target-org");
    await fill("#sa-mig-target_repository_name", `${TARGET}-copy`);
    await fill("#sa-mig-target_api_endpoint", "https://ghe.example.com/api/v3");
    await fill("#sa-mig-pat_name", "migration-token");
    await clickText("Create migration", "button[type=submit]");
    await waitText("Migration created.");
    const row = sel(`sa-mig-${me()}/${TARGET}`);
    await waitIn(row, "created");
    await clickIn(row, "start");
    await waitText("Migration start requested.");
    await waitIn(row, "failed");
    await shot("admin-17-live-migrations");
  });

  // ---------- keys and tokens ----------
  await step("admin: SSH keys and access tokens listings", async () => {
    await nav("/site-admin/tokens");
    await waitIn(sel("sa-tokens"), LOGIN);
    await nav("/site-admin/keys");
    await page().waitForSelector(sel("sa-keys"));
  });

  // ---------- audit log ----------
  await step("admin: audit log shows the actions, filters and exports", async () => {
    await nav("/site-admin/audit-log");
    await fill("#sa-audit-q", `action:user.suspend user:${LOGIN2}`);
    await clickIn(".sa-filter", "Search");
    await waitIn(sel("sa-audit"), `e2e reason ${RUN}`);
    for (const action of ["user.promote", "user.demote", "user.unsuspend", "business.maintenance_on", "business.maintenance_off", "business.set_announcement", "hook.create", "pre_receive_hook.create", "pre_receive_hook.destroy", "user.create", "user.delete"]) {
      const r = await api(`/enterprises/${ent}/audit-log?phrase=${encodeURIComponent(`action:${action}`)}&per_page=5`);
      assert(r.status === 200 && r.json.length > 0, `audit event ${action}`);
    }
    await page().select("#sa-audit-quick", "action:business");
    await waitIn(sel("sa-audit"), "business.maintenance_off");
    await shot("admin-16-audit-log");
    const client = await page().createCDPSession();
    await client.send("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: join(WORK, "downloads") }).catch(() => undefined);
    mkdirSync(join(WORK, "downloads"), { recursive: true });
    await click(sel("sa-audit-export-csv"));
    await waitText("Export downloaded.");
  });

  // ---------- mobile ----------
  await step("admin: 375px pages have no horizontal overflow", async () => {
    const m = await t.page.browserContext().newPage();
    m.setDefaultNavigationTimeout(60000);
    await m.setViewport({ width: 375, height: 800, isMobile: true, hasTouch: true });
    const pages = [
      ["admin-m-overview", "/site-admin", "sa-overview"],
      ["admin-m-users", "/site-admin/users", "sa-users"],
      ["admin-m-user", `/site-admin/users/${LOGIN2}`, "sa-user-detail"],
      ["admin-m-maintenance", "/site-admin/maintenance", "sa-maintenance"],
      ["admin-m-settings", "/site-admin/settings", "sa-settings"],
      ["admin-m-hooks", "/site-admin/hooks/new", "sa-hook-editor"],
      ["admin-m-prehooks", "/site-admin/pre-receive-hooks/new", "sa-prehook-form"],
      ["admin-m-audit", "/site-admin/audit-log", "sa-audit"],
      ["admin-m-teams", "/site-admin/enterprise-teams", "sa-ent-teams"],
      ["admin-m-repos", "/site-admin/repositories", "sa-repos"],
      ["admin-m-nodes", "/site-admin/nodes", "sa-nodes"],
      ["admin-m-repo-prehooks", `/${me()}/${TARGET}/settings/pre-receive-hooks`, "repo-prehooks"],
    ];
    for (const [name, path, id] of pages) {
      await goto(path, m);
      await m.waitForSelector(sel(id));
      const over = await m.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      await shot(name, m);
      assert(over <= 1, `${path} overflows by ${over}px`);
    }
    await m.close();
    // the maintenance page on a phone
    try {
      await api("/manage/v1/maintenance", { method: "POST", body: { enabled: true, maintenance_mode_message: `Phone check ${RUN}` } });
      await p2.setViewport({ width: 375, height: 800 });
      await goto("/", p2);
      await p2.waitForSelector(sel("maintenance-page"));
      const over = await p2.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      await shot("admin-m-maintenance-page", p2);
      assert(over <= 1, `maintenance page overflows by ${over}px`);
    } finally {
      await maintenanceOff();
      await p2.setViewport({ width: 1280, height: 900 });
    }
    await goto("/", p2);
    await waitText("Welcome", p2);
  });
}

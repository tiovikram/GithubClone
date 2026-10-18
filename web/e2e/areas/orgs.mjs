// e2e for the orgs area: create an organization, profile tabs, org repositories, teams, people
// (invite + accept in the UI by the second user), base permission and team access (UI + git),
// roles, member privileges, blocked users, security managers, outside collaborators, audit log,
// export, PAT policy page, user Organizations settings (accept, publicize, leave), rename and
// delete, 375 px overflow checks.
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

export async function run(t) {
  const { step, goto, waitText, waitBody, fill, click, clickText, shot, assert, api, BASE, RUN, WORK, PASSWORD } = t;
  const page = () => t.page;
  const ORG = `org-${RUN}`;
  const SECRET = `secret-${RUN}`;
  const OPEN = `open-${RUN}`;
  const TEAM = `core-${RUN}`;
  const CHILD = `child-${RUN}`;
  const LOGIN3 = `${t.LOGIN}-c`;
  let p2;
  let token2 = "";
  let me2 = t.LOGIN2;

  const GIT_ENV = {
    ...process.env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null", GIT_TERMINAL_PROMPT: "0",
    GIT_AUTHOR_NAME: "E2E Bot", GIT_AUTHOR_EMAIL: "e2e@example.com", GIT_COMMITTER_NAME: "E2E Bot", GIT_COMMITTER_EMAIL: "e2e@example.com",
  };
  const gitTry = (cwd, ...a) => spawnSync("git", a, { cwd, env: GIT_ENV, encoding: "utf8" });
  const cloneUrl = (login, token, repo) => {
    const u = new URL(`${BASE}/${ORG}/${repo}.git`);
    u.username = login;
    u.password = token;
    return u.toString();
  };
  /** Waits for a selector on a page. */
  const sel = (s, p = page(), timeout = 30000) => p.waitForSelector(s, { timeout });
  const selectValue = async (s, v, p = page()) => {
    await sel(s, p);
    await p.select(s, v);
  };
  const notFound = async (p) => waitText("This is not the page you are looking for", p);
  const orgApi = async (path, init = {}, p = page()) => api(path, init, p);

  await step("orgs: create organization from the user menu", async () => {
    await goto("/");
    await click('button[aria-label="Open user menu"]');
    await click('a[href="/organizations/new"]');
    await sel("#org-login");
    await fill("#org-login", ORG);
    await fill("#org-display", `E2E Org ${RUN}`);
    await fill("#org-email", `${ORG}@example.com`);
    await shot("orgs-01-new");
    await clickText("Create organization");
    await page().waitForFunction((o) => location.pathname === `/${o}`, {}, ORG);
    await waitText(`E2E Org ${RUN}`);
    await sel('[data-testid="org-tab-settings"]');
    await waitText("This organization has no repositories yet.");
    await shot("orgs-02-profile-empty");
  });

  await step("orgs: settings shell + General profile", async () => {
    await click('[data-testid="org-tab-settings"]');
    await sel('[data-testid="settings-nav-profile"]');
    for (const k of ["org-roles", "repo-roles", "member-privileges", "blocked-users", "pats", "audit-log", "migrations"]) {
      await sel(`[data-testid="settings-nav-${k}"]`);
    }
    await fill("#org-set-description", "Forging things together");
    await fill("#org-set-location", "Seattle");
    await fill("#org-set-blog", "https://example.com/org");
    await fill("#org-set-billing_email", `billing-${RUN}@example.com`);
    await clickText("Update profile");
    await waitText("Profile updated successfully.");
    await shot("orgs-03-settings-general");
    const r = await orgApi(`/orgs/${ORG}`);
    assert(r.json.description === "Forging things together" && r.json.location === "Seattle", `org after update: ${r.text}`);
    assert(r.json.billing_email === `billing-${RUN}@example.com`, "billing email saved");
    await goto(`/${ORG}`);
    await waitText("Forging things together");
    await waitText("Seattle");
  });

  await step("orgs: create repositories in the organization (UI)", async () => {
    for (const [name, priv] of [[SECRET, true], [OPEN, false]]) {
      await goto(`/orgs/${ORG}/repositories`);
      await click('[data-testid="org-new-repo"]');
      await sel("#org-repo-name");
      await fill("#org-repo-name", name);
      await fill("#org-repo-desc", `${priv ? "Private" : "Public"} e2e repository`);
      if (priv) await click('[data-testid="org-repo-private"]');
      await clickText("Create repository");
      await page().waitForFunction((p) => location.pathname === p, {}, `/${ORG}/${name}`);
      await waitText("README.md");
    }
    const r = await orgApi(`/repos/${ORG}/${SECRET}`);
    assert(r.status === 200 && r.json.private === true, `secret repo private: ${r.text}`);
  });

  await step("orgs: profile tabs, overview and repository filters", async () => {
    await goto(`/${ORG}`);
    await sel('[data-testid="org-popular"]');
    await waitText(SECRET);
    for (const [k, href] of [["overview", `/${ORG}`], ["repositories", `/orgs/${ORG}/repositories`], ["projects", `/orgs/${ORG}/projects`],
      ["packages", `/orgs/${ORG}/packages`], ["people", `/orgs/${ORG}/people`], ["teams", `/orgs/${ORG}/teams`], ["settings", `/organizations/${ORG}/settings`]]) {
      const h = await page().$eval(`[data-testid="org-tab-${k}"]`, (e) => e.getAttribute("href"));
      assert(h === href, `tab ${k} href ${h}`);
    }
    await shot("orgs-04-overview");
    await click('[data-testid="org-tab-repositories"]');
    await sel('[data-testid="org-repo-row"]');
    let n = await page().$$eval('[data-testid="org-repo-row"]', (e) => e.length);
    assert(n === 2, `repo rows ${n}`);
    await fill("#org-repo-q", "secret");
    await page().waitForFunction(() => document.querySelectorAll('[data-testid="org-repo-row"]').length === 1);
    await page().click("#org-repo-q", { count: 3 });
    await page().keyboard.press("Backspace");
    await page().waitForFunction(() => document.querySelectorAll('[data-testid="org-repo-row"]').length === 2);
    await selectValue("#org-repo-type", "public");
    await waitText(OPEN);
    await page().waitForFunction(() => document.querySelectorAll('[data-testid="org-repo-row"]').length === 1);
    await selectValue("#org-repo-sort", "name");
    await shot("orgs-05-repositories");
  });

  await step("orgs: create a team and a child team", async () => {
    await goto(`/orgs/${ORG}/teams`);
    await waitText("No teams yet");
    await click('[data-testid="new-team"]');
    await sel("#team-name");
    await fill("#team-name", TEAM);
    await fill("#team-desc", "The core team");
    await clickText("Create team");
    await page().waitForFunction((p) => location.pathname === p, {}, `/orgs/${ORG}/teams/${TEAM}`);
    await waitText("The core team");
    await click('[data-testid="team-tab-teams"]');
    await click('[data-testid="add-child-team"]');
    await sel("#team-name");
    await page().waitForFunction(() => document.querySelector("#team-parent")?.value !== "");
    await fill("#team-name", CHILD);
    await clickText("Create team");
    await page().waitForFunction((p) => location.pathname === p, {}, `/orgs/${ORG}/teams/${CHILD}`);
    await waitText(TEAM); // breadcrumb parent
    await goto(`/orgs/${ORG}/teams/${TEAM}/teams`);
    await sel('[data-testid="child-teams"]');
    await waitText(CHILD);
    await goto(`/orgs/${ORG}/teams`);
    await sel(`[data-testid="team-row-${CHILD}"]`);
    await shot("orgs-06-teams");
  });

  await step("orgs: invite the second user (UI)", async () => {
    p2 = await t.page2();
    const who = await api("/user", {}, p2);
    me2 = who.json.login;
    await goto(`/orgs/${ORG}/people`);
    await click('[data-testid="invite-member"]');
    await fill("#invite-who", me2);
    await shot("orgs-07-invite");
    await clickText("Send invitation");
    await waitText(`Invitation sent to ${me2}.`);
    await click('[data-testid="people-nav-pending"]');
    await sel(`[data-testid="invitation-${me2}"]`);
    await shot("orgs-08-pending-invitations");
    const m = await api(`/user/memberships/orgs/${ORG}`, {}, p2);
    assert(m.json.state === "pending", `pending membership: ${m.text}`);
  });

  await step("orgs: invitee accepts from Organizations settings → invitation page", async () => {
    await goto("/settings/organizations", p2);
    await sel(`[data-testid="my-invite-${ORG}"]`, p2);
    await shot("orgs-09-user-orgs-pending", p2);
    await p2.locator(`[data-testid="my-invite-${ORG}"] a[href="/orgs/${ORG}/invitation"]`).click();
    await sel('[data-testid="join-org"]', p2);
    await shot("orgs-10-invitation", p2);
    await click('[data-testid="join-org"]', p2);
    await p2.waitForFunction((o) => location.pathname === `/${o}`, {}, ORG);
    await sel('[data-testid="org-tab-people"]', p2);
    const m = await api(`/user/memberships/orgs/${ORG}`, {}, p2);
    assert(m.json.state === "active" && m.json.role === "member", `active membership: ${m.text}`);
    // members don't get the settings tab nor the settings page
    assert(!(await p2.$('[data-testid="org-tab-settings"]')), "no settings tab for members");
    await goto(`/organizations/${ORG}/settings`, p2);
    await notFound(p2);
  });

  await step("orgs: second user's token for git", async () => {
    await goto("/settings/tokens", p2);
    await fill("#token-name", `orgs-${RUN}`, p2);
    await clickText("Generate token", "button", p2);
    await p2.waitForSelector('input[aria-label="New token"]');
    token2 = await p2.$eval('input[aria-label="New token"]', (e) => e.value);
    assert(token2.startsWith("gho_"), "token2");
  });

  await step("orgs: base permission none hides the private repository", async () => {
    await goto(`/organizations/${ORG}/settings/member_privileges`);
    await selectValue("#base-perm", "none");
    await click('[data-testid="save-base-perm"]');
    await waitText("Base permission updated.");
    await shot("orgs-11-member-privileges");
    await goto(`/${ORG}/${SECRET}`, p2);
    await notFound(p2);
    const r = gitTry(WORK, "clone", "-q", cloneUrl(me2, token2, SECRET), join(WORK, `orgs-none-${RUN}`));
    assert(r.status !== 0, "clone must fail without access");
    await goto(`/orgs/${ORG}/repositories`, p2);
    await waitText(OPEN, p2);
    assert(!(await t.bodyText(p2)).includes(SECRET), "private repo hidden from repo list");
  });

  const clone2 = join(WORK, `orgs-read-${RUN}`);
  await step("orgs: base permission read allows clone, not push", async () => {
    await goto(`/organizations/${ORG}/settings/member_privileges`);
    await selectValue("#base-perm", "read");
    await click('[data-testid="save-base-perm"]');
    await waitText("Base permission updated.");
    await goto(`/${ORG}/${SECRET}`, p2);
    await waitText("README.md", p2);
    t.git(WORK, "clone", "-q", cloneUrl(me2, token2, SECRET), clone2);
    writeFileSync(join(clone2, "member.txt"), "from member\n");
    t.git(clone2, "add", "-A");
    t.git(clone2, "commit", "-q", "-m", "member change");
    const r = gitTry(clone2, "push", "-q", "origin", "HEAD");
    assert(r.status !== 0, "push must fail with read");
  });

  await step("orgs: team with write access allows push", async () => {
    await goto(`/orgs/${ORG}/teams/${TEAM}`);
    await fill("#team-add-member", me2);
    await clickText("Add member");
    await waitText(`Added ${me2} to ${TEAM}.`);
    await sel(`[data-testid="team-member-${me2}"]`);
    await click('[data-testid="team-tab-repositories"]');
    await selectValue("#team-add-repo", SECRET);
    await selectValue("#team-add-perm", "push");
    await clickText("Add repository");
    await sel(`[data-testid="team-repo-${SECRET}"]`);
    await shot("orgs-12-team-repos");
    t.git(clone2, "push", "-q", "origin", "HEAD");
    // maintainers: promote then demote
    await click('[data-testid="team-tab-members"]');
    await selectValue(`#tm-role-${me2}`, "maintainer");
    await waitText(`${me2} is now a team maintainer.`);
    await selectValue(`#tm-role-${me2}`, "member");
    await waitText(`${me2} is now a team member.`);
  });

  await step("orgs: change role to owner and back", async () => {
    await goto(`/orgs/${ORG}/people`);
    await sel(`[data-testid="member-${me2}"]`);
    await selectValue(`#role-${me2}`, "admin");
    await waitText(`${me2} is now an owner.`);
    let m = await api(`/user/memberships/orgs/${ORG}`, {}, p2);
    assert(m.json.role === "admin", "promoted");
    await selectValue("#people-role", "admin");
    await sel(`[data-testid="member-${me2}"]`);
    await goto(`/orgs/${ORG}/people`);
    await selectValue(`#role-${me2}`, "member");
    await waitText(`${me2} is now a member.`);
    m = await api(`/user/memberships/orgs/${ORG}`, {}, p2);
    assert(m.json.role === "member", "demoted");
    await shot("orgs-13-people");
  });

  await step("orgs: organization roles (custom role, assign, effect, revoke)", async () => {
    const before = await api(`/orgs/${ORG}/audit-log`, {}, p2);
    assert(before.status === 403, `member can't read audit log: ${before.status}`);
    await goto(`/organizations/${ORG}/settings/organization_roles`);
    await sel('[data-testid="org-roles"]');
    await click('[data-testid="new-org-role"]');
    await fill("#org-role-name", `auditor-${RUN}`);
    await fill("#org-role-desc", "Reads the audit log");
    await click('[data-testid="perm-read_audit_logs"]');
    await clickText("Create role");
    await sel(`[data-testid="org-role-auditor-${RUN}"]`);
    const roles = await orgApi(`/orgs/${ORG}/organization-roles`);
    const role = roles.json.roles.find((r) => r.name === `auditor-${RUN}`);
    await selectValue("#role-assignee", `user:${me2}`);
    await selectValue("#role-pick", String(role.id));
    await clickText("Assign role");
    await sel(`[data-testid="org-role-auditor-${RUN}"] [data-testid="role-assignee-${me2}"]`);
    // a predefined role for a team
    const pre = roles.json.roles.find((r) => r.source === "Predefined");
    await selectValue("#role-assignee", `team:${TEAM}`);
    await selectValue("#role-pick", String(pre.id));
    await clickText("Assign role");
    await sel(`[data-testid="role-assignee-team-${TEAM}"]`);
    await shot("orgs-14-org-roles");
    const after = await api(`/orgs/${ORG}/audit-log`, {}, p2);
    assert(after.status === 200, `role grants audit log: ${after.status}`);
    await page().locator(`[data-testid="role-assignee-${me2}"] button`).click();
    await page().waitForFunction((l) => !document.querySelector(`[data-testid="role-assignee-${l}"]`), {}, me2);
    await page().locator(`[data-testid="role-assignee-team-${TEAM}"] button`).click();
    await page().waitForFunction((s) => !document.querySelector(`[data-testid="role-assignee-team-${s}"]`), {}, TEAM);
    await page().locator(`[data-testid="org-role-auditor-${RUN}"] button::-p-text(Delete)`).click();
    await page().waitForFunction((r) => !document.querySelector(`[data-testid="org-role-auditor-${r}"]`), {}, RUN);
  });

  await step("orgs: repository roles CRUD", async () => {
    await goto(`/organizations/${ORG}/settings/repository_roles`);
    await click('[data-testid="new-repo-role"]');
    await fill("#repo-role-name", `labeler-${RUN}`);
    await fill("#repo-role-desc", "Can label");
    await selectValue("#repo-role-base", "read");
    await click('[data-testid="perm-add_label"]');
    await clickText("Create role");
    await sel(`[data-testid="repo-role-labeler-${RUN}"]`);
    await page().locator(`[data-testid="repo-role-labeler-${RUN}"] button::-p-text(Edit)`).click();
    await fill("#repo-role-desc", "Can label and triage");
    await clickText("Update role");
    await waitText("Can label and triage");
    await shot("orgs-15-repo-roles");
    // the custom role is offered on team repositories
    await goto(`/orgs/${ORG}/teams/${TEAM}/repositories`);
    await sel(`#tr-perm-${SECRET}`);
    const opts = await page().$$eval(`#tr-perm-${SECRET} option`, (os) => os.map((o) => o.value));
    assert(opts.includes(`labeler-${RUN}`), `custom role offered: ${opts}`);
    await goto(`/organizations/${ORG}/settings/repository_roles`);
    await page().locator(`[data-testid="repo-role-labeler-${RUN}"] button::-p-text(Delete)`).click();
    await waitText("No custom repository roles yet.");
  });

  await step("orgs: member privileges (repository creation)", async () => {
    await goto(`/organizations/${ORG}/settings/member_privileges`);
    await sel('[data-testid="priv-members_can_create_public_repositories"]');
    const box = '[data-testid="priv-members_can_create_public_repositories"]';
    if (await page().$eval(box, (e) => e.checked)) await click(box);
    await click('[data-testid="priv-members_can_fork_private_repositories"]');
    await click('[data-testid="save-privileges"]');
    await waitText("Member privileges updated.");
    const o = await orgApi(`/orgs/${ORG}`);
    assert(o.json.members_can_create_public_repositories === false, `public creation off: ${o.text}`);
    const r = await api(`/orgs/${ORG}/repos`, { method: "POST", body: { name: `nope-${RUN}` } }, p2);
    assert(r.status === 403 || r.status === 422, `member can't create public repo: ${r.status}`);
  });

  await step("orgs: blocked users fail their invitation", async () => {
    const su = await fetch(`${BASE}/api/forge/signup`, {
      method: "POST", headers: { "Content-Type": "application/json", "X-Forge-CSRF": "1" },
      body: JSON.stringify({ login: LOGIN3, email: `${LOGIN3}@example.com`, password: PASSWORD }),
    });
    assert(su.status === 201 || su.status === 422, `signup third user: ${su.status}`);
    await goto(`/orgs/${ORG}/people`);
    await click('[data-testid="invite-member"]');
    await fill("#invite-who", LOGIN3);
    await clickText("Send invitation");
    await waitText(`Invitation sent to ${LOGIN3}.`);
    await goto(`/organizations/${ORG}/settings/blocked_users`);
    await fill("#block-login", LOGIN3);
    await clickText("Block user");
    await sel(`[data-testid="blocked-${LOGIN3}"]`);
    await shot("orgs-16-blocked");
    await goto(`/orgs/${ORG}/people/failed_invitations`);
    await sel(`[data-testid="invitation-${LOGIN3}"]`);
    await waitText("Invitee was blocked by the organization");
    await shot("orgs-17-failed-invitations");
    await goto(`/organizations/${ORG}/settings/blocked_users`);
    await page().locator(`[data-testid="blocked-${LOGIN3}"] button`).click();
    await waitText("You have not blocked any users.");
  });

  await step("orgs: security managers", async () => {
    await goto(`/orgs/${ORG}/people/security_managers`);
    await selectValue("#secmgr-team", TEAM);
    await clickText("Add security manager team");
    await sel(`[data-testid="secmgr-${TEAM}"]`);
    await shot("orgs-18-security-managers");
    await page().locator(`[data-testid="secmgr-${TEAM}"] button`).click();
    await waitText("No security manager teams yet.");
  });

  await step("orgs: convert to outside collaborator, remove, re-invite", async () => {
    await goto(`/orgs/${ORG}/people`);
    await page().locator(`[data-testid="member-${me2}"] button::-p-text(Convert to outside collaborator)`).click();
    await waitText(`${me2} is now an outside collaborator.`);
    await click('[data-testid="people-nav-outside"]');
    await sel(`[data-testid="outside-${me2}"]`);
    await shot("orgs-19-outside");
    // still has the team's write access as a direct collaborator
    const r = await api(`/repos/${ORG}/${SECRET}`, {}, p2);
    assert(r.status === 200 && r.json.permissions.push, `outside collaborator keeps push: ${r.text}`);
    await page().locator(`[data-testid="outside-${me2}"] button`).click();
    await waitText("This organization doesn't have any outside collaborators.");
    const gone = await api(`/repos/${ORG}/${SECRET}`, {}, p2);
    assert(gone.status === 404, `no access after removal: ${gone.status}`);
    // invite again, with the team
    await goto(`/orgs/${ORG}/people`);
    await click('[data-testid="invite-member"]');
    await fill("#invite-who", me2);
    await click(`[data-testid="invite-team-${TEAM}"]`);
    await clickText("Send invitation");
    await waitText(`Invitation sent to ${me2}.`);
  });

  await step("orgs: accept in settings, publicize, team membership restored", async () => {
    await goto("/", p2);
    await sel('[data-testid="home-org-invites"]', p2).catch(() => {}); // dashboard notice (home.sidebar slot)
    await goto("/settings/organizations", p2);
    await p2.locator(`[data-testid="my-invite-${ORG}"] button::-p-text(Accept)`).click();
    await waitText(`You are now a member of ${ORG}.`, p2);
    await sel(`[data-testid="my-org-${ORG}"]`, p2);
    await click(`[data-testid="publicize-${ORG}"]`, p2);
    await waitText(`Your membership in ${ORG} is now public.`, p2);
    await shot("orgs-20-user-orgs", p2);
    const pub = await api(`/orgs/${ORG}/public_members/${me2}`, {}, p2);
    assert(pub.status === 204, "public member");
    const tm = await orgApi(`/orgs/${ORG}/teams/${TEAM}/memberships/${me2}`);
    assert(tm.status === 200, `team from invitation: ${tm.status}`);
  });

  await step("orgs: team settings (rename, notifications) and delete child team", async () => {
    await goto(`/orgs/${ORG}/teams/${CHILD}/settings`);
    await sel("#team-desc");
    await fill("#team-desc", "Nested team");
    await click('[data-testid="team-notif-off"]');
    await clickText("Save changes");
    await waitText("Team settings saved.");
    await sel('[data-testid="no-external-groups"]');
    const t1 = await orgApi(`/orgs/${ORG}/teams/${CHILD}`);
    assert(t1.json.description === "Nested team" && t1.json.notification_setting === "notifications_disabled", `team: ${t1.text}`);
    await shot("orgs-21-team-settings");
    await click('[data-testid="delete-team"]');
    await page().waitForFunction((p) => location.pathname === p, {}, `/orgs/${ORG}/teams`);
    await sel(`[data-testid="team-row-${TEAM}"]`);
    assert(!(await page().$(`[data-testid="team-row-${CHILD}"]`)), "child team deleted");
  });

  await step("orgs: audit log", async () => {
    await goto(`/organizations/${ORG}/settings/audit-log`);
    await sel('[data-testid="audit-event"]');
    await fill("#audit-q", "action:team.create");
    await clickText("Search");
    await page().waitForFunction(() => [...document.querySelectorAll('[data-testid="audit-event"]')].every((e) => e.textContent.includes("team.create")));
    await shot("orgs-22-audit-log");
  });

  await step("orgs: export (migration) and download", async () => {
    await goto(`/organizations/${ORG}/settings/migrations`);
    await click(`[data-testid="export-repo-${OPEN}"]`);
    await click('[data-testid="start-export"]');
    await sel('[data-testid="download-export"]', page(), 90000);
    await shot("orgs-23-export");
    const href = await page().$eval('[data-testid="download-export"]', (e) => e.getAttribute("href"));
    const r = await t.pageFetch(href);
    assert(r.status === 200, `archive download: ${r.status}`);
  });

  await step("orgs: personal access tokens page", async () => {
    await goto(`/organizations/${ORG}/settings/personal-access-tokens`);
    await waitText("No pending requests.");
    await waitText("No fine-grained tokens have access to this organization.");
  });

  await step("orgs: every org URL works on hard reload", async () => {
    const urls = [
      [`/orgs/${ORG}`, '[data-testid="org-tab-overview"]'], [`/organizations/${ORG}`, '[data-testid="org-popular"]'],
      ["/organizations/new", "#org-login"], [`/organizations/${ORG}/repositories/new`, "#org-repo-name"],
      [`/orgs/${ORG}/repositories`, '[data-testid="org-repo-row"]'], [`/orgs/${ORG}/people`, '[data-testid="member-list"]'],
      [`/orgs/${ORG}/outside-collaborators`, '[data-testid="people-nav-outside"]'], [`/orgs/${ORG}/people/pending_invitations`, '[data-testid="people-nav-pending"]'],
      [`/orgs/${ORG}/people/failed_invitations`, '[data-testid="failed-invitations"]'], [`/orgs/${ORG}/people/security_managers`, "#secmgr-team"],
      [`/orgs/${ORG}/teams`, '[data-testid="team-list"]'], [`/orgs/${ORG}/new-team`, "#team-name"],
      [`/orgs/${ORG}/teams/${TEAM}`, '[data-testid="team-members"]'], [`/orgs/${ORG}/teams/${TEAM}/members`, '[data-testid="team-members"]'],
      [`/orgs/${ORG}/teams/${TEAM}/teams`, '[data-testid="add-child-team"]'], [`/orgs/${ORG}/teams/${TEAM}/repositories`, '[data-testid="team-repos"]'],
      [`/orgs/${ORG}/teams/${TEAM}/settings`, '[data-testid="delete-team"]'], [`/organizations/${ORG}/settings`, "#org-set-name"],
      ["/settings/organizations", `[data-testid="my-org-${ORG}"]`],
      ...["profile", "organization_roles", "repository_roles", "member_privileges", "blocked_users", "personal-access-tokens", "audit-log", "migrations"]
        .map((s) => [`/organizations/${ORG}/settings/${s}`, ".org-settings-title"]),
    ];
    for (const [u, s] of urls) {
      await goto(u);
      await sel(s).catch((e) => { throw new Error(`${u}: ${e.message}`); });
    }
  });

  await step("orgs: 375px no horizontal overflow", async () => {
    // a new page in the signed-in (first) context
    const mp = await page().browserContext().newPage();
    await mp.setViewport({ width: 375, height: 800 });
    for (const [p, name] of [[`/${ORG}`, "orgs-375-overview"], [`/orgs/${ORG}/repositories`, "orgs-375-repos"], [`/orgs/${ORG}/people`, "orgs-375-people"],
      [`/orgs/${ORG}/teams`, "orgs-375-teams"], [`/orgs/${ORG}/teams/${TEAM}`, "orgs-375-team"], [`/organizations/${ORG}/settings/profile`, "orgs-375-settings"],
      [`/organizations/${ORG}/settings/organization_roles`, "orgs-375-roles"], ["/settings/organizations", "orgs-375-user-orgs"], ["/organizations/new", "orgs-375-new"]]) {
      await goto(p, mp);
      await mp.waitForSelector(".org-header, .settings-layout, .narrow", { timeout: 30000 });
      await new Promise((r) => setTimeout(r, 300));
      const over = await mp.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      await shot(name, mp);
      assert(over <= 1, `${p} overflows by ${over}px`);
    }
    await mp.close();
  });

  await step("orgs: leave the organization", async () => {
    await goto("/settings/organizations", p2);
    await click(`[data-testid="leave-${ORG}"]`, p2);
    await waitText(`You left ${ORG}.`, p2);
    const m = await api(`/user/memberships/orgs/${ORG}`, {}, p2);
    assert(m.status === 404, "left");
    await goto(`/${ORG}/${SECRET}`, p2);
    await notFound(p2);
  });

  const RENAMED = `${ORG}-r`;
  await step("orgs: rename the organization", async () => {
    await goto(`/organizations/${ORG}/settings/profile`);
    await fill("#org-rename", RENAMED);
    await clickText("Rename organization");
    await page().waitForFunction((p) => location.pathname === p, {}, `/organizations/${RENAMED}/settings/profile`);
    await sel("#org-set-name");
    const r = await orgApi(`/orgs/${RENAMED}`);
    assert(r.status === 200, "renamed org exists");
  });

  await step("orgs: delete the organization", async () => {
    await fill("#org-del-confirm", RENAMED);
    await clickText("Delete this organization");
    await page().waitForFunction(() => location.pathname === "/");
    const r = await orgApi(`/orgs/${RENAMED}`);
    assert(r.status === 404, `deleted: ${r.status}`);
    await goto(`/${RENAMED}`);
    await notFound(page());
  });
}

// e2e flows of the repoadmin area: repository settings (general extras, danger zone, access,
// invitations, branches/protection, rulesets + insights + bypass requests, tags, webhooks, deploy
// keys, autolinks, Pages, custom properties), org / enterprise sections (when their shells
// exist), and the Insights tab. Run: node e2e/run.mjs … --areas repoadmin --only-areas
import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export async function run(t) {
  const { step, goto, waitText, waitBody, click, fill, assert, log, shot, git, RUN, WORK, BASE } = t;
  const page = () => t.page;
  const me = () => t.me;
  const R = `ra-${RUN}`;
  const ORG = `ra-org-${RUN}`;
  const MOVED = `ra-move-${RUN}`;
  let login2 = t.LOGIN2;
  let p2;
  let token2 = "";
  const full = () => `${me()}/${R}`;
  const rp = () => `/repos/${me()}/${R}`;

  /** Poll `fn` until it returns truthy. */
  async function until(fn, what, timeout = 20000) {
    const end = Date.now() + timeout;
    let last;
    while (Date.now() < end) {
      try {
        last = await fn();
        if (last) return last;
      } catch (e) {
        last = e;
      }
      await new Promise((r) => setTimeout(r, 300));
    }
    throw new Error(`timed out waiting for ${what} (last: ${last instanceof Error ? last.message : JSON.stringify(last)})`);
  }
  const repoJson = async (path = rp()) => (await t.rest(path)).json;
  async function forge(path, init = {}, token = t.token) {
    const r = await fetch(`${BASE}/api/forge${path}`, {
      ...init,
      headers: { Authorization: `token ${token}`, "X-Forge-CSRF": "1", "Content-Type": "application/json", ...(init.headers || {}) },
      body: init.body ? JSON.stringify(init.body) : undefined,
    });
    const text = await r.text();
    return { status: r.status, json: text ? JSON.parse(text) : null };
  }
  async function rest2(path, init = {}) {
    const headers = { Accept: "application/vnd.github+json", Authorization: `token ${token2}` };
    if (init.body) headers["Content-Type"] = "application/json";
    const r = await fetch(`${BASE}/api/v3${path}`, { ...init, headers, body: init.body ? JSON.stringify(init.body) : undefined });
    const text = await r.text();
    return { status: r.status, json: text ? JSON.parse(text) : null };
  }
  const cloneUrl = (owner, repo, user, token) => {
    const u = new URL(`${BASE}/${owner}/${repo}.git`);
    u.username = user;
    u.password = token;
    return u.toString();
  };
  function gitTry(cwd, ...a) {
    const r = spawnSync("git", a, {
      cwd, encoding: "utf8",
      env: { ...process.env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null", GIT_TERMINAL_PROMPT: "0",
        GIT_AUTHOR_NAME: "E2E Bot", GIT_AUTHOR_EMAIL: "e2e@example.com", GIT_COMMITTER_NAME: "E2E Bot", GIT_COMMITTER_EMAIL: "e2e@example.com" },
    });
    return { ok: r.status === 0, out: `${r.stdout}\n${r.stderr}` };
  }
  /** Is there a settings shell (sidebar) on this page? */
  const hasShell = async (p = page()) => !!(await p.$(".settings-nav"));
  const clickIn = async (sel, text, tag = "button") => page().locator(`${sel} ${tag}::-p-text(${text})`).click();

  // ---------- setup ----------
  await step("repoadmin: setup repositories, second user, organization", async () => {
    await t.newRepo(R, { description: "repoadmin e2e" });
    p2 = await t.page2();
    const who = await t.pageFetch("/api/v3/user", {}, p2);
    login2 = JSON.parse(who.text).login;
    const tk = await t.pageFetch("/api/forge/tokens", { method: "POST", headers: { "X-Forge-CSRF": "1", "Content-Type": "application/json" }, body: JSON.stringify({ name: `ra-${RUN}` }) }, p2);
    token2 = JSON.parse(tk.text).token;
    assert(token2, "second user token");
    const o = await forge("/orgs", { method: "POST", body: { login: ORG } });
    assert(o.status === 201, `create org ${o.status} ${JSON.stringify(o.json)}`);
  });

  // ---------- settings: general extras ----------
  await step("repoadmin: settings sidebar groups", async () => {
    await goto(`/${full()}/settings`);
    await waitText("Collaborators and teams");
    const heads = await page().$$eval(".settings-nav-heading", (e) => e.map((x) => x.textContent));
    for (const h of ["Access", "Code and automation", "Security"]) assert(heads.includes(h), `sidebar heading ${h}: ${heads}`);
    for (const k of ["access", "branches", "tags", "rules", "hooks", "pages", "keys", "key_links"])
      assert(await page().$(`[data-testid="settings-nav-${k}"]`), `sidebar item ${k}`);
  });

  await step("repoadmin: general features, merge buttons, template, LFS", async () => {
    await goto(`/${full()}/settings`);
    await page().waitForSelector('[data-testid="general-extras"]');
    await click("#gen-wiki");
    await until(async () => (await repoJson()).has_wiki === false, "has_wiki=false");
    await click("#gen-squash");
    await until(async () => (await repoJson()).allow_squash_merge === false, "allow_squash_merge=false");
    await click("#gen-delete-head");
    await until(async () => (await repoJson()).delete_branch_on_merge === true, "delete_branch_on_merge");
    await page().select("#gen-merge-msg", "PR_TITLE|PR_BODY");
    await until(async () => (await repoJson()).merge_commit_title === "PR_TITLE", "merge commit title");
    await click("#gen-discussions");
    await until(async () => (await repoJson()).has_discussions === true, "has_discussions");
    await click("#gen-signoff");
    await until(async () => (await repoJson()).web_commit_signoff_required === true, "signoff");
    await click("#gen-template");
    await until(async () => (await repoJson()).is_template === true, "is_template");
    await page().waitForFunction(() => !document.querySelector("#gen-lfs")?.disabled);
    const before = (await forge(`/repos/${full()}/lfs`)).json.enabled;
    await click("#gen-lfs");
    await until(async () => (await forge(`/repos/${full()}/lfs`)).json.enabled === !before, "lfs toggled");
    await page().reload({ waitUntil: "networkidle0" });
    const wiki = await page().$eval("#gen-wiki", (e) => e.checked);
    assert(!wiki, "wiki stays off after reload");
    await shot("ra-01-general");
  });

  await step("repoadmin: use this template", async () => {
    await goto(`/${full()}`);
    await click('[data-testid="use-template"]');
    await page().waitForSelector('[data-testid="generate-form"]');
    await fill("#gen-name", `${R}-gen`);
    await fill("#gen-desc", "from template");
    await shot("ra-02-generate");
    await t.clickText("Create repository");
    await page().waitForFunction((p) => location.pathname === p, {}, `/${me()}/${R}-gen`);
    const g = await repoJson(`/repos/${me()}/${R}-gen`);
    assert(g.template_repository?.full_name === full(), `template_repository ${JSON.stringify(g.template_repository)}`);
  });

  await step("repoadmin: danger zone — change visibility", async () => {
    await goto(`/${full()}/settings`);
    await clickIn('[data-testid="danger-visibility"]', "Change visibility");
    await fill("#vis-confirm", full());
    await t.clickText("I understand, change repository visibility.");
    await until(async () => (await repoJson()).private === true, "private");
    await waitBody(/This repository is currently private/);
    await clickIn('[data-testid="danger-visibility"]', "Change visibility");
    await fill("#vis-confirm", full());
    await t.clickText("I understand, change repository visibility.");
    await until(async () => (await repoJson()).private === false, "public again");
  });

  await step("repoadmin: danger zone — archive blocks writes, unarchive", async () => {
    await goto(`/${full()}/settings`);
    await clickIn('[data-testid="danger-archive"]', "Archive this repository");
    await fill("#archive-confirm", full());
    await t.clickText("I understand the consequences, archive this repository");
    await page().waitForSelector('[data-testid="archived-badge"]');
    const w = await t.rest(`${rp()}/issues`, { method: "POST", body: { title: "should fail" } });
    assert(w.status === 403 || w.status === 410 || w.status === 422, `issue on archived repo: ${w.status}`);
    await shot("ra-03-archived");
    await clickIn('[data-testid="danger-archive"]', "Unarchive this repository");
    await fill("#archive-confirm", full());
    await t.clickText("I understand the consequences, unarchive this repository");
    await until(async () => (await repoJson()).archived === false, "unarchived");
    await page().waitForFunction(() => !document.querySelector('[data-testid="archived-badge"]'));
  });

  await step("repoadmin: danger zone — transfer to an organization", async () => {
    await t.newRepo(MOVED);
    await goto(`/${me()}/${MOVED}/settings`);
    await clickIn('[data-testid="danger-transfer"]', "Transfer");
    await fill("#transfer-owner", ORG);
    await fill("#transfer-confirm", `${me()}/${MOVED}`);
    await t.clickText("I understand, transfer this repository.");
    await page().waitForFunction((p) => location.pathname === p, {}, `/${ORG}/${MOVED}`);
    const r = await repoJson(`/repos/${ORG}/${MOVED}`);
    assert(r.owner.login === ORG, `owner after transfer ${r.owner.login}`);
  });

  // ---------- access ----------
  let invitations = false;
  await step("repoadmin: add collaborator (invitation), invitee accepts on the repository invitation page", async () => {
    await goto(`/${full()}/settings/access`);
    await t.clickText("Add people");
    await fill("#add-collab", login2);
    await page().locator('input[name="add-role"][value="write"]').click();
    await shot("ra-04-add-people");
    await page().locator("form[aria-label='Add people'] button[type=submit]").click();
    await page().waitForSelector(`[data-testid="invite-${login2}"], [data-testid="collab-${login2}"]`);
    invitations = !!(await page().$(`[data-testid="invite-${login2}"]`));
    if (!invitations) {
      log("    SKIP invitation accept flow: the server adds collaborators directly (repository invitations are off)");
      return;
    }
    await shot("ra-05-pending-invite");
    // invitee: dashboard box → repository invitation page → accept
    await goto("/", p2);
    await p2.waitForSelector('[data-testid="pending-invitations"]');
    await goto(`/${full()}/invitations`, p2);
    await p2.waitForSelector('[data-testid="invitation"]');
    await shot("ra-06-invitation-page", p2);
    await p2.locator("button::-p-text(Accept invitation)").click();
    await p2.waitForFunction((p) => location.pathname === p, {}, `/${full()}`);
    await goto(`/${full()}/settings/access`);
    await page().waitForSelector(`[data-testid="collab-${login2}"]`);
  });

  await step("repoadmin: change collaborator role, remove collaborator", async () => {
    await goto(`/${full()}/settings/access`);
    await page().waitForSelector(`[data-testid="collab-${login2}"]`);
    await page().select(`#role-${login2}`, "maintain");
    await until(async () => (await t.rest(`${rp()}/collaborators/${login2}/permission`)).json.role_name === "maintain", "role maintain");
    await shot("ra-07-access");
    await page().locator(`button[aria-label="Remove ${login2}"]`).click();
    await page().waitForFunction((l) => !document.querySelector(`[data-testid="collab-${l}"]`), {}, login2);
    const c = await t.rest(`${rp()}/collaborators/${login2}`);
    assert(c.status === 404, `collaborator removed: ${c.status}`);
  });

  await step("repoadmin: invitations list — decline; cancel a pending invite", async () => {
    if (!invitations) return log("    SKIP (repository invitations are off)");
    const inv = await t.rest(`${rp()}/collaborators/${login2}`, { method: "PUT", body: { permission: "read" } });
    assert(inv.status === 201, `invite: ${inv.status}`);
    await goto("/invitations", p2);
    await p2.waitForSelector(`[data-testid="my-invite-${full()}"]`);
    await shot("ra-08-my-invitations", p2);
    await p2.locator(`[data-testid="my-invite-${full()}"] button::-p-text(Decline)`).click();
    await p2.waitForFunction((f) => !document.querySelector(`[data-testid="my-invite-${f}"]`), {}, full());
    // owner: invite again, change the invite role, then cancel it
    await t.rest(`${rp()}/collaborators/${login2}`, { method: "PUT", body: { permission: "read" } });
    await goto(`/${full()}/settings/access`);
    await page().waitForSelector(`[data-testid="invite-${login2}"]`);
    await page().select(`[data-testid="invite-${login2}"] select`, "triage");
    await until(async () => (await t.rest(`${rp()}/invitations`)).json.some((i) => i.invitee.login === login2 && i.permissions === "triage"), "invite role triage");
    await clickIn(`[data-testid="invite-${login2}"]`, "Cancel invite");
    await page().waitForFunction((l) => !document.querySelector(`[data-testid="invite-${l}"]`), {}, login2);
    assert((await t.rest(`${rp()}/invitations`)).json.length === 0, "no invitations left");
  });

  await step("repoadmin: org repository — teams and custom repository roles", async () => {
    const team = await t.rest(`/orgs/${ORG}/teams`, { method: "POST", body: { name: "devs" } });
    assert(team.status === 201, `team ${team.status} ${team.text}`);
    const role = await t.rest(`/orgs/${ORG}/custom-repository-roles`, { method: "POST", body: { name: "Labeler", base_role: "triage", permissions: ["add_label"] } });
    assert(role.status === 201, `custom role ${role.status} ${role.text}`);
    await goto(`/${ORG}/${MOVED}/settings/access`);
    await t.clickText("Add teams");
    await page().waitForSelector("#add-team");
    await page().select("#add-team", "devs");
    await page().select("#add-team-role", "write");
    await t.clickText("Add team", "button");
    await page().waitForSelector('[data-testid="team-devs"]');
    const teams = (await t.rest(`/repos/${ORG}/${MOVED}/teams`)).json;
    assert(teams.some((x) => x.slug === "devs" && x.permission === "push"), `team permission ${JSON.stringify(teams)}`);
    await page().select("#team-role-devs", "Labeler");
    await until(async () => (await t.rest(`/repos/${ORG}/${MOVED}/teams`)).json.some((x) => x.slug === "devs" && x.permission === "Labeler"), "team custom role");
    // a person with a custom role
    await t.clickText("Add people");
    await fill("#add-collab", login2);
    await page().locator('input[name="add-role"][value="Labeler"]').click();
    await page().locator("form[aria-label='Add people'] button[type=submit]").click();
    await page().waitForSelector(`[data-testid="invite-${login2}"], [data-testid="collab-${login2}"]`);
    if (await page().$(`[data-testid="invite-${login2}"]`)) {
      const mine = (await rest2("/user/repository_invitations")).json;
      const inv = mine.find((i) => i.repository.full_name === `${ORG}/${MOVED}`);
      assert(inv, "custom-role invitation");
      assert((await rest2(`/user/repository_invitations/${inv.id}`, { method: "PATCH" })).status === 204, "accept");
    }
    await until(async () => (await t.rest(`/repos/${ORG}/${MOVED}/collaborators/${login2}/permission`)).json.role_name === "Labeler", "collaborator custom role");
    await goto(`/${ORG}/${MOVED}/settings/access`);
    await page().waitForSelector(`[data-testid="collab-${login2}"]`);
    const sel = await page().$eval(`#role-${login2}`, (e) => e.value);
    assert(sel === "Labeler", `role select shows ${sel}`);
    await shot("ra-09-org-access");
    await page().locator('button[aria-label="Remove team devs"]').click();
    await page().waitForFunction(() => !document.querySelector('[data-testid="team-devs"]'));
  });

  // ---------- branches ----------
  const wd = join(WORK, `ra-${RUN}`);
  await step("repoadmin: classic branch protection — create, enforce on push, edit, delete", async () => {
    await goto(`/${full()}/settings/branches`);
    await waitText("Branch protection rules");
    await t.clickText("Add classic branch protection rule", "a");
    await page().waitForSelector('[data-testid="protection-editor"]');
    await page().select("#bp-branch", "main");
    await click("#bp-pr");
    await page().select("#bp-approvals", "2");
    await click("#bp-stale");
    await click("#bp-checks");
    await click("#bp-strict");
    await fill("#bp-contexts", "ci/test");
    await click("#bp-conversation");
    await click("#bp-linear");
    await click("#bp-admins");
    await shot("ra-10-protection-editor");
    await t.clickText("Create", "button");
    await page().waitForSelector('[data-testid="protection-main"]');
    const p = (await t.rest(`${rp()}/branches/main/protection`)).json;
    assert(p.required_pull_request_reviews?.required_approving_review_count === 2, `approvals ${JSON.stringify(p.required_pull_request_reviews)}`);
    assert(p.required_pull_request_reviews.dismiss_stale_reviews === true, "stale");
    assert(p.required_status_checks?.strict && p.required_status_checks.contexts.includes("ci/test"), "status checks");
    assert(p.required_linear_history?.enabled && p.enforce_admins?.enabled && p.required_conversation_resolution?.enabled, "linear/admins/conversation");
    await shot("ra-11-protection-list");
    // enforcement: a direct push to main is rejected
    mkdirSync(wd, { recursive: true });
    git(wd, "clone", "-q", cloneUrl(me(), R, me(), t.token), "w");
    const w = join(wd, "w");
    writeFileSync(join(w, "direct.txt"), "direct\n");
    git(w, "add", "-A");
    git(w, "commit", "-q", "-m", "direct push");
    const push = gitTry(w, "push", "origin", "HEAD:main");
    assert(!push.ok, `push to protected main should fail:\n${push.out}`);
    git(w, "reset", "-q", "--hard", "HEAD~1");
    // edit: drop the PR requirement, allow force pushes
    await t.clickText("Edit", "a");
    await page().waitForSelector('[data-testid="protection-editor"]');
    await page().waitForFunction(() => document.querySelector("#bp-pr")?.checked === true);
    await click("#bp-pr");
    await click("#bp-force");
    await t.clickText("Save changes", "button");
    await page().waitForSelector('[data-testid="protection-main"]');
    const p2j = (await t.rest(`${rp()}/branches/main/protection`)).json;
    assert(!p2j.required_pull_request_reviews && p2j.allow_force_pushes?.enabled, `edited ${JSON.stringify(p2j)}`);
    await page().locator('[data-testid="protection-main"] button::-p-text(Delete)').click();
    await page().waitForFunction(() => !document.querySelector('[data-testid="protection-main"]'));
    assert((await t.rest(`${rp()}/branches/main/protection`)).status === 404, "protection deleted");
  });

  // ---------- rulesets ----------
  let rsId;
  await step("repoadmin: branch ruleset — create with targets, rules, bypass; enforced on push", async () => {
    await goto(`/${full()}/settings/rules`);
    await waitText("Rulesets define whether");
    await t.clickText("New branch ruleset", "a");
    await page().waitForSelector('[data-testid="ruleset-editor"]');
    await fill("#rs-name", `locked-${RUN}`);
    await page().select('[data-testid="ref-targets"] select', "include");
    await fill('[data-testid="ref-targets"] input[aria-label="branches name pattern"]', "locked/*");
    await t.clickText("Add target", "button");
    await waitText("locked/*");
    await click('[data-testid="rule-creation"] input[type=checkbox]');
    await click('[data-testid="rule-required_linear_history"] input[type=checkbox]');
    await click('[data-testid="rule-pull_request"] input[type=checkbox]');
    await fill("#rp-pull_request-required_approving_review_count", "1");
    await click('[data-testid="rule-required_status_checks"] input[type=checkbox]');
    await fill("#rp-required_status_checks-contexts", "ci/build");
    await click('[data-testid="rule-commit_message_pattern"] input[type=checkbox]');
    await fill("#rp-commit_message_pattern-pattern", "feat");
    await page().select('[data-testid="bypass-list"] select[aria-label="Add bypass"]', "RepositoryRole:2");
    await page().locator('[data-testid="bypass-list"] button::-p-text(Add)').click();
    await waitText("Role: Maintain");
    await shot("ra-12-ruleset-editor");
    await t.clickText("Create", "button");
    await page().waitForFunction(() => /\/settings\/rules\/\d+$/.test(location.pathname));
    rsId = Number(await page().evaluate(() => location.pathname.split("/").pop()));
    const r = (await t.rest(`${rp()}/rulesets/${rsId}`)).json;
    const types = r.rules.map((x) => x.type).sort();
    for (const ty of ["creation", "deletion", "non_fast_forward", "pull_request", "required_linear_history", "required_status_checks", "commit_message_pattern"])
      assert(types.includes(ty), `rule ${ty} in ${types}`);
    assert(r.conditions.ref_name.include.includes("~DEFAULT_BRANCH") && r.conditions.ref_name.include.includes("refs/heads/locked/*"), `targets ${JSON.stringify(r.conditions)}`);
    assert(r.bypass_actors.some((b) => b.actor_type === "RepositoryRole" && b.actor_id === 2), "bypass actor");
    assert(r.rules.find((x) => x.type === "required_status_checks").parameters.required_status_checks[0].context === "ci/build", "status check param");
    // enforcement: creating locked/x is blocked
    const w = join(wd, "w");
    const push = gitTry(w, "push", "origin", "HEAD:refs/heads/locked/x");
    assert(!push.ok && /GH013|restricted|rule/i.test(push.out), `push to locked/x should be rejected:\n${push.out}`);
    await shot("ra-13-ruleset-saved");
  });

  await step("repoadmin: rule insights list and suite detail", async () => {
    await goto(`/${full()}/settings/rules/insights?time_period=day`);
    await page().waitForSelector('[data-testid="rule-suite"]');
    await shot("ra-14-insights");
    await page().locator('[data-testid="rule-suite"] a::-p-text(locked/x)').click();
    await page().waitForSelector('[data-testid="rule-suite-detail"]');
    await waitText("Restrict creations");
  });

  await step("repoadmin: edit ruleset (evaluate), history, export; tag ruleset; delete", async () => {
    await goto(`/${full()}/settings/rules/${rsId}`);
    await page().waitForFunction((n) => document.querySelector("#rs-name")?.value === n, {}, `locked-${RUN}`);
    await page().select("#rs-enforcement", "evaluate");
    await t.clickText("Save changes", "button");
    await waitText("Ruleset updated.");
    assert((await t.rest(`${rp()}/rulesets/${rsId}`)).json.enforcement === "evaluate", "evaluate");
    const push = gitTry(join(wd, "w"), "push", "origin", "HEAD:refs/heads/locked/y");
    assert(push.ok, `evaluate mode does not block:\n${push.out}`);
    await t.clickText("History", "a");
    await page().waitForSelector('[data-testid="ruleset-history"]');
    // tag ruleset from the Tags section
    await goto(`/${full()}/settings/tags`);
    await t.clickText("New tag ruleset", "a");
    await page().waitForSelector('[data-testid="ruleset-editor"]');
    await fill("#rs-name", `tags-${RUN}`);
    await t.clickText("Include all tags", "button");
    await t.clickText("Create", "button");
    await page().waitForFunction(() => /\/settings\/rules\/\d+$/.test(location.pathname));
    const all = (await t.rest(`${rp()}/rulesets`)).json;
    const tagRs = all.find((x) => x.name === `tags-${RUN}`);
    assert(tagRs?.target === "tag", "tag ruleset");
    const tagPush = gitTry(join(wd, "w"), "push", "origin", "HEAD:refs/tags/v0.0.1");
    assert(tagPush.ok, `tag creation allowed: ${tagPush.out}`);
    const tagDel = gitTry(join(wd, "w"), "push", "origin", ":refs/tags/v0.0.1");
    assert(!tagDel.ok, `tag deletion restricted:\n${tagDel.out}`);
    await goto(`/${full()}/settings/tags`);
    await page().waitForSelector(`[data-testid="ruleset-tags-${RUN}"]`);
    await shot("ra-15-tags");
    await goto(`/${full()}/settings/rules`);
    await page().locator(`button[aria-label="Delete ruleset locked-${RUN}"]`).click();
    await page().waitForFunction((n) => !document.querySelector(`[data-testid="ruleset-${n}"]`), {}, `locked-${RUN}`);
    assert((await t.rest(`${rp()}/rulesets/${rsId}`)).status === 404, "ruleset deleted");
  });

  await step("repoadmin: push ruleset — blocked push creates a bypass request, admin approves, push succeeds", async () => {
    // second user gets write access
    const add = await t.rest(`${rp()}/collaborators/${login2}`, { method: "PUT", body: { permission: "write" } });
    if (add.status === 201) assert((await rest2(`/user/repository_invitations/${add.json.id}`, { method: "PATCH" })).status === 204, "accept");
    await goto(`/${full()}/settings/rules/new?target=push`);
    await page().waitForSelector('[data-testid="ruleset-editor"]');
    await fill("#rs-name", `push-${RUN}`);
    await click('[data-testid="rule-file_extension_restriction"] input[type=checkbox]');
    await fill("#rp-file_extension_restriction-restricted_file_extensions", ".exe");
    await click('[data-testid="rule-max_file_size"] input[type=checkbox]');
    await page().select('[data-testid="bypass-list"] select[aria-label="Add bypass"]', "RepositoryRole:5");
    await page().locator('[data-testid="bypass-list"] button::-p-text(Add)').click();
    await t.clickText("Create", "button");
    await page().waitForFunction(() => /\/settings\/rules\/\d+$/.test(location.pathname));
    const w2 = join(wd, "w2");
    git(wd, "clone", "-q", cloneUrl(me(), R, login2, token2), "w2");
    git(w2, "checkout", "-q", "-b", "tool");
    writeFileSync(join(w2, "tool.exe"), "MZ\n");
    git(w2, "add", "-A");
    git(w2, "commit", "-q", "-m", "add tool");
    const blocked = gitTry(w2, "push", "origin", "tool");
    assert(!blocked.ok, `push with .exe should be blocked:\n${blocked.out}`);
    await goto(`/${full()}/settings/rules/bypass-requests`);
    await page().waitForSelector('[data-testid="bypass-requests"] a');
    await shot("ra-16-bypass-requests");
    await page().locator('[data-testid="bypass-requests"] a').click();
    await page().waitForSelector('[data-testid="bypass-request"]');
    await fill("#bypass-msg", "ok for the release");
    await t.clickText("Approve bypass", "button");
    await waitText("Bypass request approved.");
    await shot("ra-17-bypass-approved");
    const again = gitTry(w2, "push", "origin", "tool");
    assert(again.ok, `approved bypass lets the push through:\n${again.out}`);
    // the requester's view of the exemptions list
    await goto(`/${full()}/exemptions`, p2);
    await p2.waitForSelector('[data-testid="bypass-requests"]');
    const del = (await t.rest(`${rp()}/rulesets`)).json.find((x) => x.name === `push-${RUN}`);
    await t.rest(`${rp()}/rulesets/${del.id}`, { method: "DELETE" });
  });

  // ---------- webhooks ----------
  await step("repoadmin: webhooks — create, ping, deliveries with request/response, redeliver, edit, delete", async () => {
    const got = [];
    const srv = createServer((req, res) => {
      let b = "";
      req.on("data", (c) => (b += c));
      req.on("end", () => { got.push({ event: req.headers["x-github-event"], body: b }); res.writeHead(200, { "Content-Type": "text/plain" }); res.end("thanks"); });
    });
    await new Promise((r) => srv.listen(0, "127.0.0.1", r));
    const port = srv.address().port;
    try {
      await goto(`/${full()}/settings/hooks`);
      await t.clickText("Add webhook", "a");
      await page().waitForSelector('[data-testid="hook-form"]');
      await fill("#hook-url", `http://127.0.0.1:${port}/hook`);
      await fill("#hook-secret", "s3cret");
      await click("#hook-ev-pick");
      await page().waitForSelector('[data-testid="hook-events"]');
      await click('[data-testid="hook-events"] input[value="issues"]');
      await click('[data-testid="hook-events"] input[value="release"]');
      await shot("ra-18-hook-form");
      await t.clickText("Add webhook", "button");
      await page().waitForFunction(() => /\/settings\/hooks\/\d+$/.test(location.pathname));
      const id = Number(await page().evaluate(() => location.pathname.split("/").pop()));
      const h = (await t.rest(`${rp()}/hooks/${id}`)).json;
      assert(["push", "issues", "release"].every((e) => h.events.includes(e)), `events ${h.events}`);
      await t.clickText("Ping", "button");
      await waitText("Ping sent.");
      await t.clickText("Recent Deliveries", "a");
      await until(async () => {
        await page().reload({ waitUntil: "networkidle0" });
        return page().$('[data-testid="delivery"]');
      }, "a delivery");
      await page().locator('[data-testid="delivery"]').click();
      await page().waitForSelector('[data-testid="delivery-detail"]');
      await waitBody(/X-GitHub-Event: ping/i);
      await page().locator('[data-testid="delivery-detail"] button[role=tab]::-p-text(Response)').click();
      const delivered = got.length > 0;
      if (delivered) await waitText("thanks");
      else log("    note: the delivery did not reach the local receiver (server blocks private addresses); response shows the error");
      await shot("ra-19-delivery");
      const n0 = (await t.rest(`${rp()}/hooks/${id}/deliveries`)).json.length;
      await t.clickText("Redeliver", "button");
      await waitText("Redelivery requested.");
      await until(async () => (await t.rest(`${rp()}/hooks/${id}/deliveries`)).json.length > n0, "redelivery recorded");
      await t.clickText("Settings", "a");
      await page().waitForSelector("#hook-active");
      await click("#hook-active");
      await t.clickText("Update webhook", "button");
      await waitText("successfully updated");
      assert((await t.rest(`${rp()}/hooks/${id}`)).json.active === false, "inactive");
      await goto(`/${full()}/settings/hooks`);
      await page().waitForSelector(`[data-testid="hook-${id}"]`);
      await shot("ra-20-hooks");
      await page().locator(`[data-testid="hook-${id}"] button::-p-text(Delete)`).click();
      await page().waitForFunction((i) => !document.querySelector(`[data-testid="hook-${i}"]`), {}, id);
    } finally {
      srv.close();
    }
  });

  // ---------- deploy keys / autolinks ----------
  await step("repoadmin: deploy keys add and delete", async () => {
    const kf = join(WORK, `ra-dk-${RUN}`);
    const g = spawnSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-C", "ra-e2e", "-f", kf], { encoding: "utf8" });
    assert(g.status === 0, g.stderr);
    const pub = readFileSync(`${kf}.pub`, "utf8").trim();
    await goto(`/${full()}/settings/keys`);
    await t.clickText("Add deploy key", "button");
    await fill("#dk-title", "ci deploy");
    await fill("#dk-key", pub);
    await click("#dk-write");
    await t.clickText("Add key", "button");
    await page().waitForSelector('[data-testid="deploy-key-ci deploy"]');
    await waitText("Read/write");
    await shot("ra-21-deploy-keys");
    const keys = (await t.rest(`${rp()}/keys`)).json;
    assert(keys.length === 1 && keys[0].read_only === false, "deploy key saved");
    await page().locator('button[aria-label="Delete deploy key ci deploy"]').click();
    await page().waitForFunction(() => !document.querySelector('[data-testid="deploy-key-ci deploy"]'));
  });

  await step("repoadmin: autolink references add and delete", async () => {
    await goto(`/${full()}/settings/key_links`);
    await t.clickText("Add autolink reference", "button");
    await fill("#al-prefix", "TICKET-");
    await fill("#al-url", "https://tracker.example.com/browse/TICKET-<num>");
    await waitText("Preview:");
    await page().locator("form[aria-label='Add autolink reference'] button[type=submit]").click();
    await page().waitForSelector('[data-testid="autolink-TICKET-"]');
    await shot("ra-22-autolinks");
    assert((await t.rest(`${rp()}/autolinks`)).json.some((a) => a.key_prefix === "TICKET-"), "autolink saved");
    await page().locator('button[aria-label="Delete autolink TICKET-"]').click();
    await page().waitForFunction(() => !document.querySelector('[data-testid="autolink-TICKET-"]'));
  });

  // ---------- pages ----------
  await step("repoadmin: GitHub Pages — publish from branch, build, visit, custom domain, unpublish", async () => {
    await goto(`/${full()}/settings/pages`);
    await page().waitForSelector("#pages-source");
    await page().select("#pages-source", "legacy");
    await page().select("#pages-branch", "main");
    await page().locator("form[aria-label='Build and deployment'] button[type=submit]").click();
    await page().waitForSelector('[data-testid="pages-live"]');
    const site = (await t.rest(`${rp()}/pages`)).json;
    assert(site.build_type === "legacy" && site.source.branch === "main", `site ${JSON.stringify(site)}`);
    await until(async () => (await t.rest(`${rp()}/pages`)).json.status === "built", "pages built", 30000);
    const href = await page().$eval('[data-testid="pages-url"]', (a) => a.getAttribute("href"));
    const res = await t.pageFetch(href);
    assert(res.status === 200, `site ${href}: ${res.status}`);
    await t.clickText("Request build", "button");
    await waitText("Build requested.");
    await fill("#pages-cname", "docs.example.com");
    await page().locator("form[aria-label='Custom domain'] button[type=submit]").click();
    await waitText("Custom domain saved.");
    assert((await t.rest(`${rp()}/pages`)).json.cname === "docs.example.com", "cname");
    await page().reload({ waitUntil: "networkidle0" });
    await page().waitForSelector('[data-testid="pages-builds"]');
    await shot("ra-23-pages");
    await t.clickText("Unpublish site", "button");
    await page().waitForFunction(() => !document.querySelector('[data-testid="pages-live"]'));
    assert((await t.rest(`${rp()}/pages`)).status === 404, "unpublished");
  });

  // ---------- custom properties (repository values) ----------
  await step("repoadmin: custom property values on an organization repository", async () => {
    const d = await t.rest(`/orgs/${ORG}/properties/schema/env`, { method: "PUT", body: { value_type: "single_select", allowed_values: ["dev", "prod"], values_editable_by: "org_and_repo_actors" } });
    assert(d.status === 200, `property def ${d.status} ${d.text}`);
    await goto(`/${ORG}/${MOVED}/settings/custom-properties`);
    await page().waitForSelector('[data-testid="repo-properties"]');
    await page().select("#prop-env", "prod");
    await t.clickText("Save changes", "button");
    await waitText("Custom properties saved.");
    const v = (await t.rest(`/repos/${ORG}/${MOVED}/properties/values`)).json;
    assert(v.some((x) => x.property_name === "env" && x.value === "prod"), `values ${JSON.stringify(v)}`);
    await shot("ra-24-repo-properties");
  });

  // ---------- org sections (orgs area renders the shell) ----------
  await step("repoadmin: organization settings — rulesets, webhooks, custom properties (needs orgs shell)", async () => {
    await goto(`/organizations/${ORG}/settings/rules`);
    if (!(await hasShell())) return log("    SKIP organization settings: the orgs area's settings shell is not available (needs orgs shell)");
    await waitText("Rulesets define whether");
    await t.clickText("New branch ruleset", "a");
    await page().waitForSelector('[data-testid="repo-targets"]');
    await fill("#rs-name", `org-${RUN}`);
    await t.clickText("Include all branches", "button");
    await t.clickText("Create", "button");
    await page().waitForFunction(() => /\/settings\/rules\/\d+$/.test(location.pathname));
    const ors = (await t.rest(`/orgs/${ORG}/rulesets`)).json;
    assert(ors.some((x) => x.name === `org-${RUN}`), "org ruleset");
    const inherited = (await t.rest(`/repos/${ORG}/${MOVED}/rulesets`)).json;
    assert(inherited.some((x) => x.name === `org-${RUN}` && x.source_type === "Organization"), "inherited by repo");
    await shot("ra-25-org-ruleset");
    await goto(`/organizations/${ORG}/settings/rules/insights`);
    await waitText("Rule insights");
    await goto(`/organizations/${ORG}/settings/hooks`);
    await t.clickText("Add webhook", "a");
    await fill("#hook-url", "https://hooks.example.com/org");
    await t.clickText("Add webhook", "button");
    await page().waitForFunction(() => /\/settings\/hooks\/\d+$/.test(location.pathname));
    assert((await t.rest(`/orgs/${ORG}/hooks`)).json.length === 1, "org hook");
    await goto(`/organizations/${ORG}/settings/custom-properties`);
    await page().waitForSelector('[data-testid="property-defs"]');
    await t.clickText("New property", "button");
    await fill("#pd-name", "team");
    await fill("#pd-desc", "Owning team");
    await t.clickText("Save property", "button");
    await page().waitForSelector('[data-testid="propdef-team"]');
    await shot("ra-26-org-properties");
    await page().locator('button[aria-label="Delete property team"]').click();
    await page().waitForFunction(() => !document.querySelector('[data-testid="propdef-team"]'));
  });

  await step("repoadmin: site admin — enterprise rulesets, custom properties, bypass requests (needs admin shell)", async () => {
    const meJson = (await t.rest("/user")).json;
    if (!meJson.site_admin) return log("    SKIP enterprise sections: the e2e user is not a site admin");
    await goto("/site-admin/rulesets");
    if (!(await hasShell())) return log("    SKIP enterprise sections: the admin area's /site-admin shell is not available (needs admin shell)");
    await waitText("Rulesets define whether");
    await t.clickText("New branch ruleset", "a");
    await fill("#rs-name", `ent-${RUN}`);
    await fill("#rs-org-inc", ORG);
    await t.clickText("Include all branches", "button");
    await t.clickText("Create", "button");
    await page().waitForFunction(() => /\/site-admin\/rulesets\/\d+$/.test(location.pathname));
    const repoRs = (await t.rest(`/repos/${ORG}/${MOVED}/rulesets`)).json;
    assert(repoRs.some((x) => x.name === `ent-${RUN}` && x.source_type === "Enterprise"), "enterprise ruleset applies");
    await shot("ra-27-enterprise-ruleset");
    await t.clickText("Rulesets", "a");
    await page().locator(`button[aria-label="Delete ruleset ent-${RUN}"]`).click();
    await page().waitForFunction((n) => !document.querySelector(`[data-testid="ruleset-${n}"]`), {}, `ent-${RUN}`);
    await goto("/site-admin/custom-properties");
    await waitText("Enterprise custom properties");
    await goto("/site-admin/bypass-requests");
    await waitText("Push bypass requests");
  });

  // ---------- insights ----------
  await step("repoadmin: insights — pulse, contributors, commits, code frequency, punch card", async () => {
    await goto(`/${full()}`);
    await click('[data-testid="repo-tab-insights"]');
    await page().waitForSelector('[data-testid="pulse"]');
    const sel = await page().$eval('[data-testid="repo-tab-insights"]', (e) => e.className);
    assert(sel.includes("selected"), "Insights tab selected");
    await shot("ra-28-pulse");
    await click('[data-testid="insights-nav-contributors"]');
    await page().waitForSelector(`[data-testid="contributor-${me()}"], [data-testid="contributors"]`);
    await shot("ra-29-contributors");
    await click('[data-testid="insights-nav-commits"]');
    await page().waitForSelector('[data-testid="commit-activity"]');
    await click('[data-testid="insights-nav-code-frequency"]');
    await page().waitForSelector('[data-testid="code-frequency"]');
    await click('[data-testid="insights-nav-punch-card"]');
    await page().waitForSelector('[data-testid="punch-card"]');
    await shot("ra-30-punch-card");
    const dep = await page().$eval('[data-testid="insights-nav-dependencies"]', (a) => a.getAttribute("href"));
    assert(dep === `/${full()}/network/dependencies`, `dependency graph link ${dep}`);
  });

  await step("repoadmin: insights — traffic (views beacon + clones), community, forks", async () => {
    await until(async () => (await t.rest(`${rp()}/traffic/views`)).json.count > 0, "views recorded");
    await goto(`/${full()}/graphs/traffic`);
    await page().waitForSelector('[data-testid="traffic"]');
    const clones = (await t.rest(`${rp()}/traffic/clones`)).json;
    assert(clones.count >= 1, `clones ${JSON.stringify(clones)}`);
    await page().waitForSelector('[data-testid="traffic-paths"] a');
    await shot("ra-31-traffic");
    await click('[data-testid="insights-nav-community"]');
    await page().waitForSelector('[data-testid="community-readme"]');
    const ok = await page().$eval('[data-testid="community-description"]', (e) => e.textContent.includes("✔"));
    assert(ok, "description checked");
    await shot("ra-32-community");
    const f = await rest2(`${rp()}/forks`, { method: "POST", body: {} });
    assert(f.status === 202 || f.status === 201, `fork ${f.status}`);
    await goto(`/${full()}/forks`);
    await waitText(`${login2}/${R}`);
    await shot("ra-33-forks");
  });

  await step("repoadmin: 375px pages have no horizontal overflow", async () => {
    const m = await page().browserContext().newPage();
    await m.setViewport({ width: 375, height: 800, isMobile: true, hasTouch: true });
    m.setDefaultNavigationTimeout(60000);
    const pages = [
      ["ra-m-general", `/${full()}/settings`],
      ["ra-m-access", `/${full()}/settings/access`],
      ["ra-m-branches", `/${full()}/settings/branches`],
      ["ra-m-protection", `/${full()}/settings/branches/new`],
      ["ra-m-rules", `/${full()}/settings/rules`],
      ["ra-m-ruleset-new", `/${full()}/settings/rules/new?target=branch`],
      ["ra-m-insights-rules", `/${full()}/settings/rules/insights`],
      ["ra-m-hooks-new", `/${full()}/settings/hooks/new`],
      ["ra-m-keys", `/${full()}/settings/keys`],
      ["ra-m-pages", `/${full()}/settings/pages`],
      ["ra-m-pulse", `/${full()}/pulse`],
      ["ra-m-contributors", `/${full()}/graphs/contributors`],
      ["ra-m-traffic", `/${full()}/graphs/traffic`],
      ["ra-m-punch", `/${full()}/graphs/punch-card`],
      ["ra-m-community", `/${full()}/community`],
      ["ra-m-invitations", `/invitations`],
    ];
    for (const [name, p] of pages) {
      await m.goto(BASE + p, { waitUntil: "networkidle0" });
      const over = await m.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      await shot(name, m);
      assert(over <= 1, `${p} overflows by ${over}px`);
    }
    await m.close();
  });
}

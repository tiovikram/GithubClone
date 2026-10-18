// Security area e2e: Security tab (overview, policy, advisories + private reporting, Dependabot,
// code scanning, secret scanning with push protection and delegated bypass), dependency graph and
// SBOM export, dependency review on PRs, Advanced Security and Dependabot secrets settings, org
// code security / security overview / Dependabot secrets, site admin code security, and the
// advisory database browser. Offline: advisories come from the OSV fixture of the server tests,
// secrets are random fake AWS key ids, code scanning results a small SARIF file.
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURE = join(HERE, "../../../crates/forge/tests/fixtures/advisories/osv-sample.json");

function fakeAwsKey() {
  const A = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let k = "AKIA";
  for (let i = 0; i < 16; i++) k += A[Math.floor(Math.random() * A.length)];
  return k;
}
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

export async function run(t) {
  const { step, goto, waitText, waitBody, click, fill, clickText, shot, assert, log } = t;
  const page = () => t.page;
  const me = t.me;
  const REPO = `sec-${t.RUN}`;
  const full = `${me}/${REPO}`;
  const base = `/${full}`;
  const ORG = `secorg-${t.RUN}`;
  let advisoriesReady = false;
  let isAdmin = false;
  let defaultSha = "";

  /** Reload `path` until `selector` appears (background jobs: scans, matching, SARIF). */
  async function waitFor(path, selector, timeout = 60000) {
    const end = Date.now() + timeout;
    for (;;) {
      await goto(path);
      if (await page().$(selector)) return;
      if (Date.now() > end) throw new Error(`timed out waiting for ${selector} on ${path}`);
      await new Promise((r) => setTimeout(r, 1500));
    }
  }
  async function noOverflow(label) {
    const w = await page().evaluate(() => document.documentElement.scrollWidth);
    assert(w <= 375, `${label}: page is ${w}px wide at 375px`);
  }
  async function select(sel, value) {
    await page().locator(sel).wait();
    await page().select(sel, value);
  }
  async function putFile(path, content, message, branch) {
    const cur = await t.rest(`/repos/${full}/contents/${path}${branch ? `?ref=${branch}` : ""}`);
    const r = await t.rest(`/repos/${full}/contents/${path}`, {
      method: "PUT", body: { message, content: b64(content), branch, sha: cur.status === 200 ? cur.json.sha : undefined },
    });
    assert(r.status === 200 || r.status === 201, `put ${path}: ${r.status} ${r.text}`);
    return r.json;
  }

  await step("security: setup repository, advisories and fixtures", async () => {
    const who = await t.rest("/user");
    isAdmin = !!who.json.site_admin;
    if (isAdmin) {
      const r = await fetch(`${t.BASE}/api/forge/advisories`, {
        method: "POST", headers: { Authorization: `token ${t.token}`, "Content-Type": "application/json" }, body: readFileSync(FIXTURE, "utf8"),
      });
      assert(r.ok, `import advisories: ${r.status}`);
    }
    advisoriesReady = (await t.rest("/advisories/GHSA-p6mc-m468-83gw")).status === 200;
    if (!advisoriesReady) log("    SKIP Dependabot alert steps: advisory fixture not imported (sign in as a site admin once, or run `forge advisories sync`)");
    await t.newRepo(REPO, { description: "security e2e" });
    await putFile("package.json", JSON.stringify({ name: "sec-e2e", dependencies: { lodash: "4.17.15", "left-pad": "1.3.0" } }, null, 2), "Add package.json");
    await putFile("src/app.js", "const x = process.argv[2];\neval(x);\nmodule.exports = x;\n", "Add app");
    await putFile("config/settings.py", `AWS_KEY_ID = "${fakeAwsKey()}"\n`, "Add settings");
    const b = await t.rest(`/repos/${full}/branches/main`);
    defaultSha = b.json.commit.sha;
  });

  await step("security: tab and overview", async () => {
    await goto(base);
    await click('[data-testid="repo-tab-security"]');
    await page().waitForSelector('[data-testid="security-overview"]');
    await waitText("Security policy");
    await shot("security-01-overview");
  });

  await step("security: create a security policy", async () => {
    await click('[data-testid="sec-nav-policy"]');
    await click('[data-testid="start-policy"]');
    await page().waitForSelector('[data-testid="policy-editor"]');
    await click('[data-testid="commit-policy"]');
    await page().waitForSelector('[data-testid="security-policy"] .markdown-body h1');
    await waitText("Reporting a Vulnerability");
    await click('[data-testid="sec-nav-overview"]');
    await waitBody(/Defined in SECURITY\.md/);
    await shot("security-02-policy");
  });

  await step("security: Advanced Security settings", async () => {
    await click('[data-testid="overview-settings-link"]');
    await page().waitForSelector('[data-testid="advanced-security-settings"]');
    const enable = async (id) => {
      await page().waitForFunction((i) => !document.querySelector(`[data-testid="toggle-${i}-btn"]`)?.disabled, {}, id);
      await click(`[data-testid="toggle-${id}-btn"]`);
      await page().waitForFunction((i) => document.querySelector(`[data-testid="toggle-${i}"] .badge`)?.textContent === "Enabled", {}, id);
    };
    for (const id of ["pvr", "dependabot-alerts", "dependabot-security-updates", "secret-scanning", "push-protection"]) await enable(id);
    await click('[data-testid="default-setup-toggle"]');
    await page().waitForFunction(() => document.querySelector('[data-testid="default-setup-state"]')?.textContent === "Configured");
    // The settings sidebar links here too.
    await page().waitForSelector('[data-testid="settings-nav-security-analysis"]');
    await shot("security-03-settings");
  });

  if (advisoriesReady) {
    await step("security: Dependabot alerts list and filters", async () => {
      await waitFor(`${base}/security/dependabot`, '[data-testid="dependabot-row"]');
      await waitText("Prototype Pollution in lodash");
      await select("#db-sev", "critical");
      await page().waitForSelector('[data-testid="dependabot-empty"]');
      await select("#db-sev", "high");
      await page().waitForSelector('[data-testid="dependabot-row"]');
      await select("#db-scope", "runtime");
      await page().waitForSelector('[data-testid="dependabot-row"]');
      await clickText("Clear filters");
      await page().waitForSelector('[data-testid="dependabot-row"]');
      await shot("security-04-dependabot");
    });

    await step("security: Dependabot alert dismiss and reopen", async () => {
      await page().locator('[data-testid="dependabot-row"] a.issue-title').click();
      await page().waitForSelector('[data-testid="dependabot-alert"]');
      await waitText("4.17.19");
      await click('[data-testid="dismiss-alert"]');
      await click('input[name="dismiss-reason"][value="tolerable_risk"]');
      await fill('[data-testid="dismiss-form"] textarea', "Only used at build time");
      await click('[data-testid="dismiss-submit"]');
      await page().waitForSelector('[data-testid="dismissed-note"]');
      await waitText("Only used at build time");
      await shot("security-05-dependabot-dismissed");
      await click('[data-testid="reopen-alert"]');
      await page().waitForFunction(() => document.querySelector('[data-testid="alert-state"]')?.textContent === "Open");
      await click('[data-testid="advisory-link"]');
      await page().waitForSelector('[data-testid="global-advisory"] [data-testid="affected-package"]');
    });
  }

  await step("security: dependency graph and SBOM export", async () => {
    await goto(`${base}/security`);
    await click('[data-testid="sec-nav-dependency-graph"]');
    await page().waitForSelector('[data-testid="dep-row"]');
    await waitText("lodash");
    if (advisoriesReady) await page().waitForSelector('[data-testid="dep-vuln"]');
    const dl = join(t.WORK, "sbom");
    mkdirSync(dl, { recursive: true });
    const cdp = await page().createCDPSession();
    await cdp.send("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: dl, browserContextId: page().browserContext().id });
    await click('[data-testid="export-sbom"]');
    const end = Date.now() + 15000;
    let file;
    while (!(file = readdirSync(dl).find((f) => f.endsWith(".spdx.json"))) && Date.now() < end) await new Promise((r) => setTimeout(r, 300));
    assert(file, "SBOM downloaded");
    const sbom = JSON.parse(readFileSync(join(dl, file), "utf8"));
    assert(sbom.spdxVersion === "SPDX-2.3" && sbom.packages.some((p) => p.name === "npm:lodash"), "SBOM contents");
    await fill('[data-testid="dep-filter"]', "left");
    await page().waitForFunction(() => document.querySelectorAll('[data-testid="dep-row"]').length === 1);
    await shot("security-06-dependency-graph");
  });

  await step("security: dependency review on a pull request", async () => {
    const r = await t.rest(`/repos/${full}/git/refs`, { method: "POST", body: { ref: "refs/heads/bump-deps", sha: (await t.rest(`/repos/${full}/branches/main`)).json.commit.sha } });
    assert(r.status === 201, `branch: ${r.status}`);
    await putFile("package.json", JSON.stringify({ name: "sec-e2e", dependencies: { lodash: "4.17.15", minimist: "1.2.5" } }, null, 2), "Add minimist", "bump-deps");
    const pr = await t.rest(`/repos/${full}/pulls`, { method: "POST", body: { title: "Add minimist", head: "bump-deps", base: "main" } });
    assert(pr.status === 201, `pr: ${pr.status} ${pr.text}`);
    await goto(`${base}/pull/${pr.json.number}`);
    await page().waitForSelector('[data-testid="dependency-review"]');
    await waitBody(/1 added, 1 removed/);
    if (advisoriesReady) await page().waitForSelector('[data-testid="dependency-review-vuln"]');
    await click('[data-testid="dependency-review-toggle"]');
    await page().waitForSelector('[data-testid="dependency-review-change"]');
    await shot("security-07-dependency-review");
  });

  await step("security: secret scanning alert resolve and reopen", async () => {
    await waitFor(`${base}/security/secret-scanning`, '[data-testid="secret-row"]');
    await waitText("Amazon AWS Access Key ID");
    await page().waitForSelector('[data-testid="scan-history"]');
    await select("#ss-val", "unknown");
    await page().waitForSelector('[data-testid="secret-row"]');
    await page().locator('[data-testid="secret-row"] a.issue-title').click();
    await page().waitForSelector('[data-testid="secret-alert"]');
    await page().waitForSelector('[data-testid="secret-location"]');
    await click('[data-testid="reveal-secret"]');
    const v = await page().$eval('[data-testid="secret-value"]', (e) => e.value);
    assert(/^AKIA[A-Z2-7]{16}$/.test(v), `revealed secret: ${v}`);
    await click('[data-testid="close-alert"]');
    await click('input[name="secret-res"][value="used_in_tests"]');
    await fill('[data-testid="resolve-form"] textarea', "Test fixture");
    await click('[data-testid="resolve-submit"]');
    await page().waitForSelector('[data-testid="resolved-note"]');
    await shot("security-08-secret-alert");
    await click('[data-testid="reopen-alert"]');
    await page().waitForFunction(() => document.querySelector('[data-testid="alert-state"]')?.textContent === "Open");
  });

  let placeholderUrl = "";
  await step("security: push protection blocks a push, unblock page allows it", async () => {
    const dir = join(t.WORK, `sec-clone-${t.RUN}`);
    const url = new URL(`${t.BASE}/${full}.git`);
    url.username = me;
    url.password = t.token;
    t.git(t.WORK, "clone", "-q", url.toString(), dir);
    writeFileSync(join(dir, "deploy.env"), `AWS_ACCESS_KEY_ID=${fakeAwsKey()}\n`);
    t.git(dir, "add", "deploy.env");
    t.git(dir, "commit", "-q", "-m", "Add deploy env");
    let out = "";
    try {
      t.git(dir, "push", "-q", "origin", "HEAD:main");
      throw new Error("push with a secret was accepted");
    } catch (e) {
      out = e.message;
    }
    assert(/GH013/.test(out), `push rejected with GH013: ${out.slice(0, 400)}`);
    const m = out.match(/https?:\/\/\S+\/security\/secret-scanning\/unblock-secret\/\S+/);
    assert(m, "unblock URL in the rejection");
    placeholderUrl = new URL(m[0]).pathname;
    await goto(placeholderUrl);
    await page().waitForSelector('[data-testid="unblock-form"]');
    await click('input[name="bypass-reason"][value="false_positive"]');
    await click('[data-testid="unblock-submit"]');
    await page().waitForSelector('[data-testid="unblock-done"]');
    await shot("security-09-unblock");
    t.git(dir, "push", "-q", "origin", "HEAD:main");
    // The bypassed secret becomes an alert; the bypasses page lists it.
    const end = Date.now() + 60000;
    for (;;) {
      await goto(`${base}/security/secret-scanning/push-protection-bypasses`);
      await clickText("Bypassed secrets");
      try {
        await page().waitForSelector('[data-testid="bypassed-row"]', { timeout: 2000 });
        break;
      } catch (e) {
        if (Date.now() > end) throw e;
      }
    }
  });

  await step("security: delegated bypass request reviewed by the owner", async () => {
    const p2 = await t.page2();
    const login2 = (await t.api("/user", {}, p2)).json.login;
    const inv = await t.rest(`/repos/${full}/collaborators/${login2}`, { method: "PUT", body: { permission: "push" } });
    assert(inv.status === 201 || inv.status === 204, `collaborator: ${inv.status}`);
    if (inv.status === 201 && inv.json?.id) {
      const acc = await t.api(`/user/repository_invitations/${inv.json.id}`, { method: "PATCH" }, p2);
      assert(acc.status === 204, `accept invitation: ${acc.status}`);
    }
    // Turn delegated bypass on in the settings page.
    await goto(`${base}/settings/security_analysis`);
    await page().waitForFunction(() => !document.querySelector('[data-testid="toggle-delegated-bypass-btn"]')?.disabled);
    await click('[data-testid="toggle-delegated-bypass-btn"]');
    await page().waitForFunction(() => document.querySelector('[data-testid="toggle-delegated-bypass"] .badge')?.textContent === "Enabled");
    // The collaborator's write with a secret is blocked; their bypass becomes a request.
    const secretFile = `ci-${t.RUN}.env`;
    const content = { message: "Add CI env", content: b64(`KEY=${fakeAwsKey()}\n`) };
    const blocked = await t.api(`/repos/${full}/contents/${secretFile}`, { method: "PUT", body: content }, p2);
    assert(blocked.status === 409, `blocked write: ${blocked.status} ${blocked.text}`);
    const ph = blocked.json.errors?.[0]?.placeholder_id;
    assert(ph, "placeholder id");
    await p2.goto(`${t.BASE}${base}/security/secret-scanning/unblock-secret/${ph}`, { waitUntil: "networkidle0" });
    await p2.locator('input[name="bypass-reason"][value="will_fix_later"]').click();
    await p2.locator('[data-testid="unblock-submit"]').click();
    await p2.waitForSelector('[data-testid="unblock-requested"]');
    await shot("security-10-bypass-requested", p2);
    // The owner reviews it.
    await goto(`${base}/security/secret-scanning/push-protection-bypasses`);
    await page().locator('[data-testid="request-row"] a.issue-title').click();
    await page().waitForSelector('[data-testid="review-form"]');
    await fill('[data-testid="review-form"] textarea', "OK for the CI fixture");
    await click('[data-testid="approve-request"]');
    await page().waitForFunction(() => document.querySelector('[data-testid="request-box"] [data-testid="alert-state"]')?.textContent === "Approved");
    await shot("security-11-bypass-approved");
    const ok = await t.api(`/repos/${full}/contents/${secretFile}`, { method: "PUT", body: content }, p2);
    assert(ok.status === 201, `write after approval: ${ok.status} ${ok.text}`);
  });

  await step("security: code scanning SARIF upload, alert dismiss and reopen", async () => {
    await goto(`${base}/security/code-scanning`);
    await page().waitForSelector('[data-testid="code-empty"]');
    await click('[data-testid="code-tools-link"]');
    await page().waitForSelector('[data-testid="sarif-upload"]');
    const sarif = {
      version: "2.1.0",
      runs: [{
        tool: { driver: { name: "forge-e2e-lint", semanticVersion: "1.0.0", rules: [{
          id: "js/eval", name: "NoEval", shortDescription: { text: "Avoid eval" }, fullDescription: { text: "eval executes arbitrary code." },
          defaultConfiguration: { level: "error" }, help: { text: "Do not use eval.", markdown: "Do **not** use `eval`." },
          properties: { tags: ["security", "external/cwe/cwe-95"], "security-severity": "8.8" },
        }] } },
        results: [{ ruleId: "js/eval", level: "error", message: { text: "Untrusted input reaches eval." },
          locations: [{ physicalLocation: { artifactLocation: { uri: "src/app.js" }, region: { startLine: 2, startColumn: 1, endColumn: 9 } } }] }],
      }],
    };
    const file = join(t.WORK, "results.sarif");
    writeFileSync(file, JSON.stringify(sarif));
    await (await page().$("#sarif-file")).uploadFile(file);
    await page().waitForFunction(() => /^[0-9a-f]{40}$/.test(document.querySelector("#sarif-sha")?.value || ""));
    await click('[data-testid="sarif-submit"]');
    await page().waitForFunction(() => /complete/.test(document.querySelector('[data-testid="sarif-status"]')?.textContent || ""), { timeout: 60000 });
    await page().waitForSelector('[data-testid="analysis-row"]');
    await shot("security-12-code-tools");
    await goto(`${base}/security/code-scanning`);
    await page().waitForSelector('[data-testid="code-row"]');
    await select("#cs-tool", "forge-e2e-lint");
    await page().waitForSelector('[data-testid="code-row"]');
    await page().locator('[data-testid="code-row"] a.issue-title').click();
    await page().waitForSelector('[data-testid="code-snippet"]');
    await waitText("Untrusted input reaches eval.");
    await page().waitForSelector('[data-testid="code-snippet"] .sec-hl');
    await click('[data-testid="dismiss-alert"]');
    await click('input[name="code-reason"][value="used in tests"]');
    await click('[data-testid="dismiss-submit"]');
    await page().waitForSelector('[data-testid="dismissed-note"]');
    await shot("security-13-code-alert");
    await click('[data-testid="reopen-alert"]');
    await page().waitForFunction(() => document.querySelector('[data-testid="alert-state"]')?.textContent === "Open");
  });

  let ghsa = "";
  await step("security: draft advisory with credits, CVE request, publish", async () => {
    const login2 = (await t.api("/user", {}, await t.page2())).json.login;
    await goto(`${base}/security/advisories`);
    await click('[data-testid="new-advisory"]');
    await page().waitForSelector('[data-testid="advisory-form"]');
    await fill("#adv-title", `Command injection in sec-e2e ${t.RUN}`);
    await fill("#adv-desc", "### Impact\nUntrusted input reaches `eval`.");
    await fill("#adv-pkg-0", `sec-e2e-${t.RUN}`);
    await fill("#adv-range-0", "< 2.0.0");
    await fill("#adv-patched-0", "2.0.0");
    await select("#adv-sev", "high");
    await fill("#adv-cwe", "CWE-95");
    await click('[data-testid="add-credit"]');
    await fill("#credit-login-0", login2);
    await click('[data-testid="advisory-submit"]');
    await page().waitForSelector('[data-testid="advisory-page"]');
    ghsa = await page().evaluate(() => location.pathname.split("/").pop());
    assert(/^GHSA-/.test(ghsa), `ghsa: ${ghsa}`);
    await page().waitForSelector('[data-testid="credit"]');
    await waitBody(/\(pending\)/);
    await click('[data-testid="request-cve"]');
    await page().waitForSelector('[data-testid="cve-requested"]');
    await click('[data-testid="edit-advisory"]');
    await fill("#adv-title", `Code injection in sec-e2e ${t.RUN}`);
    await click('[data-testid="advisory-submit"]');
    await page().waitForFunction((s) => document.querySelector('[data-testid="advisory-title"]')?.textContent === s, {}, `Code injection in sec-e2e ${t.RUN}`);
    await shot("security-14-advisory-draft");
    await click('[data-testid="publish-advisory"]');
    await page().waitForFunction(() => document.querySelector('[data-testid="alert-state"]')?.textContent === "Published");
    await click('[data-testid="global-advisory-link"]');
    await page().waitForSelector('[data-testid="global-advisory"] [data-testid="affected-package"]');
    await waitText(`sec-e2e-${t.RUN}`);
    await goto(`${base}/security/advisories`);
    await page().waitForSelector('[data-testid="advisory-row"]');
  });

  await step("security: private vulnerability report accepted and closed", async () => {
    const p2 = await t.page2();
    await p2.goto(`${t.BASE}${base}/security/advisories`, { waitUntil: "networkidle0" });
    await p2.locator('[data-testid="report-vuln"]').click();
    await p2.waitForSelector('[data-testid="report-page"]');
    await p2.locator("#adv-title").fill(`XSS in the README renderer ${t.RUN}`);
    await p2.locator("#adv-desc").fill("Steps to reproduce: open the README.");
    await p2.locator("#adv-pkg-0").fill("sec-e2e");
    await p2.locator('[data-testid="advisory-submit"]').click();
    await p2.waitForSelector('[data-testid="triage-banner"]');
    await shot("security-15-report", p2);
    await goto(`${base}/security/advisories`);
    await click('[data-testid="state-tab-triage"]');
    await page().locator('[data-testid="advisory-row"] a.issue-title').click();
    await click('[data-testid="accept-report"]');
    await page().waitForFunction(() => document.querySelector('[data-testid="alert-state"]')?.textContent === "Draft");
    await click('[data-testid="close-advisory"]');
    await page().waitForFunction(() => document.querySelector('[data-testid="alert-state"]')?.textContent === "Closed");
    await page().waitForSelector('[data-testid="reopen-advisory"]');
  });

  await step("security: advisory database browser", async () => {
    await click('button[aria-label="Open user menu"]');
    await clickText("Advisory database", "a");
    await page().waitForSelector('[data-testid="global-advisories"]');
    await page().waitForSelector('[data-testid="global-advisory-row"]');
    await fill('[data-testid="advisory-search"]', ghsa || "GHSA-p6mc-m468-83gw");
    await clickText("Search", 'button[type="submit"]');
    await page().waitForFunction(() => document.querySelectorAll('[data-testid="global-advisory-row"]').length === 1);
    await fill('[data-testid="advisory-search"]', "no-such-package-xyz");
    await clickText("Search", 'button[type="submit"]');
    await page().waitForSelector('[data-testid="advisories-empty"]');
    await goto("/advisories");
    await click('[data-testid="advisory-type-unreviewed"]');
    await page().waitForFunction(() => location.search.includes("type=unreviewed"));
    await click('[data-testid="advisory-type-reviewed"]');
    await page().waitForSelector('[data-testid="global-advisory-row"]');
    await page().locator('[data-testid="global-advisory-row"] a.issue-title').click();
    await page().waitForSelector('[data-testid="global-advisory"] [data-testid="advisory-title"]');
    await shot("security-16-advisory-db");
  });

  await step("security: repository Dependabot secrets", async () => {
    await goto(`${base}/settings`);
    await click('[data-testid="settings-nav-secrets-dependabot"]');
    await click('[data-testid="new-dep-secret"]');
    await fill("#dep-secret-name", "NPM_TOKEN");
    await fill("#dep-secret-value", "npm-secret-value");
    await click('[data-testid="secret-save"]');
    await page().waitForSelector('[data-testid="dep-secret-row"]');
    await click('[data-testid="edit-dep-secret"]');
    await fill("#dep-secret-value", "rotated");
    await click('[data-testid="secret-save"]');
    await page().waitForFunction(() => !document.querySelector('[data-testid="secret-form"]'));
    await shot("security-17-dependabot-secrets");
    await click('[data-testid="delete-dep-secret"]');
    await page().waitForFunction(() => !document.querySelector('[data-testid="dep-secret-row"]'));
  });

  // ---------- organization sections (rendered by the orgs area's settings shell) ----------
  let orgShell = false;
  await step("security: organization code security, overview and Dependabot secrets", async () => {
    const r = await fetch(`${t.BASE}/api/forge/orgs`, { method: "POST", headers: { Authorization: `token ${t.token}`, "Content-Type": "application/json" }, body: JSON.stringify({ login: ORG }) });
    assert(r.status === 201, `create org: ${r.status}`);
    const repo = await t.rest(`/orgs/${ORG}/repos`, { method: "POST", body: { name: "app", auto_init: true } });
    assert(repo.status === 201, `org repo: ${repo.status}`);
    await goto(`/organizations/${ORG}/settings/security_products`);
    orgShell = !!(await page().$(".settings-nav")) && !!(await page().$('[data-testid="org-code-security"]'));
    if (!orgShell) {
      log("    SKIP organization settings steps: the org settings shell (orgs area) is not rendered on this build");
      return;
    }
    await click('[data-testid="new-config"]');
    await fill("#cfg-name", `e2e-${t.RUN}`);
    await fill("#cfg-desc", "Secret scanning everywhere");
    await select('[data-testid="cfg-secret_scanning"]', "enabled");
    await select('[data-testid="cfg-dependabot_alerts"]', "enabled");
    await click('[data-testid="config-save"]');
    await page().waitForSelector('[data-testid="config-row"]');
    await waitText(`e2e-${t.RUN}`);
    const rows = await page().$$('[data-testid="config-row"]');
    let row;
    for (const x of rows) if ((await x.evaluate((e) => e.textContent)).includes(`e2e-${t.RUN}`)) row = x;
    await (await row.$('[data-testid="apply-config"]')).click();
    await page().waitForSelector('[data-testid="attach-repos"] input');
    await page().locator('[data-testid="attach-repos"] input').click();
    await select('[data-testid="default-select"]', "all");
    await click('[data-testid="attach-submit"]');
    await page().waitForSelector('[data-testid="attach-done"]');
    await page().waitForSelector('[data-testid="attached-repos"]');
    await page().waitForSelector('[data-testid="config-default"]');
    await shot("security-18-org-configs");
    // Edit it.
    await page().locator(`a.issue-title::-p-text(e2e-${t.RUN})`).click();
    await page().waitForSelector('[data-testid="config-form"]');
    await select('[data-testid="cfg-private_vulnerability_reporting"]', "enabled");
    await click('[data-testid="config-save"]');
    await page().waitForSelector('[data-testid="config-list"]');
    // The org repository picked up the configuration.
    const cfg = await t.rest(`/repos/${ORG}/app/code-security-configuration`);
    assert(cfg.status === 200 && cfg.json.configuration.name === `e2e-${t.RUN}`, `repo configuration: ${cfg.status}`);
    await page().waitForSelector('[data-testid="pattern-row"]');
    // Security overview.
    await click('[data-testid="settings-nav-security-overview"]');
    await page().waitForSelector('[data-testid="org-security-overview"]');
    await clickText("Requests");
    await shot("security-19-org-overview");
    // Org Dependabot secrets with selected repositories.
    await click('[data-testid="settings-nav-secrets-dependabot"]');
    await click('[data-testid="new-dep-secret"]');
    await fill("#dep-secret-name", "ORG_NPM");
    await fill("#dep-secret-value", "org-secret");
    await click('input[name="dep-vis"][value="selected"]');
    await page().locator('.sec-repo-picker input').click();
    await click('[data-testid="secret-save"]');
    await page().waitForSelector('[data-testid="dep-secret-row"]');
    await waitText("Selected repositories");
    await select('[aria-label="Repository to grant access to"]', String(repo.json.id));
    await click('[data-testid="dep-access-add"]');
    await page().waitForSelector('[data-testid="dep-access-row"]');
    await shot("security-20-org-secrets");
    await click('[data-testid="delete-dep-secret"]');
    await page().waitForFunction(() => !document.querySelector('[data-testid="dep-secret-row"]'));
  });

  await step("security: site admin code security", async () => {
    await goto("/site-admin/code-security");
    const shell = !!(await page().$(".settings-nav")) && !!(await page().$('[data-testid="enterprise-code-security"]'));
    if (!shell) {
      log(`    SKIP site admin steps: ${isAdmin ? "the site admin shell (admin area) is not rendered on this build" : "the e2e user is not a site admin"}`);
      return;
    }
    await page().waitForSelector('[data-testid="enterprise-defaults"]');
    const btn = '[data-testid="toggle-ent-secret_scanning_enabled_for_new_repositories-btn"]';
    const before = await page().$eval('[data-testid="toggle-ent-secret_scanning_enabled_for_new_repositories"] .badge', (e) => e.textContent);
    await click(btn);
    await page().waitForFunction((b) => document.querySelector('[data-testid="toggle-ent-secret_scanning_enabled_for_new_repositories"] .badge')?.textContent !== b, {}, before);
    await click(btn);
    await page().waitForFunction((b) => document.querySelector('[data-testid="toggle-ent-secret_scanning_enabled_for_new_repositories"] .badge')?.textContent === b, {}, before);
    await click('[data-testid="new-config"]');
    await fill("#cfg-name", `ent-${t.RUN}`);
    await click('[data-testid="config-save"]');
    await waitText(`ent-${t.RUN}`);
    await shot("security-21-enterprise");
    const rows = await page().$$('[data-testid="config-row"]');
    for (const x of rows) {
      if ((await x.evaluate((e) => e.textContent)).includes(`ent-${t.RUN}`)) {
        await (await x.$('[data-testid="delete-config"]')).click();
      }
    }
    await page().waitForFunction((n) => !document.body.innerText.includes(n), {}, `ent-${t.RUN}`);
  });

  await step("security: delete a code scanning analysis", async () => {
    await goto(`${base}/security/code-scanning/tools`);
    await click('[data-testid="delete-analysis"]');
    await page().waitForFunction(() => !document.querySelector('[data-testid="analysis-row"]'));
    await goto(`${base}/security/code-scanning`);
    await page().waitForSelector('[data-testid="code-empty"]');
  });

  await step("security: pages fit a 375px screen", async () => {
    await page().setViewport({ width: 375, height: 800 });
    try {
      const pages = [
        `${base}/security`, `${base}/security/policy`, `${base}/security/advisories`, `${base}/security/advisories/${ghsa}`,
        `${base}/security/advisories/new`, `${base}/security/secret-scanning`, `${base}/security/secret-scanning/1`,
        `${base}/security/code-scanning`, `${base}/security/code-scanning/tools`, `${base}/network/dependencies`,
        `${base}/settings/security_analysis`, `${base}/settings/secrets/dependabot`, "/advisories", "/advisories/GHSA-p6mc-m468-83gw",
      ];
      if (advisoriesReady) pages.push(`${base}/security/dependabot`, `${base}/security/dependabot/1`);
      if (orgShell) pages.push(`/organizations/${ORG}/settings/security_products`, `/organizations/${ORG}/settings/security_overview`);
      for (const p of pages) {
        await goto(p);
        await noOverflow(p);
      }
      await goto(`${base}/security/dependabot`);
      await shot("security-22-mobile");
    } finally {
      await page().setViewport({ width: 1280, height: 900 });
    }
  });

}

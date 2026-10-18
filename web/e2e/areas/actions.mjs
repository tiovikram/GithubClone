// e2e for the actions area: a real `forge runner` (shell executor, started here as a child
// process with a token copied from the "New self-hosted runner" page) runs workflows pushed with
// git; the UI shows runs, jobs, live logs, summaries, annotations and artifacts; re-runs,
// dispatch with inputs, cancel, delete; secrets and variables created through the UI (sealed in
// the browser) are seen masked by jobs; environments with a required reviewer approved in the
// UI; checks and statuses on a PR and a commit; deployments; caches; Actions settings; runner
// labels; organization and site admin sections when their shells are present.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "../../..");

const CI = `name: CI
on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:
    inputs:
      greeting:
        description: Who to greet
        default: world
        required: true
      level:
        type: choice
        options: [low, high]
      loud:
        type: boolean
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Greet
        run: |
          echo "hello \${{ inputs.greeting || 'push' }} level=\${{ inputs.level }} loud=\${{ inputs.loud }}"
          echo "::warning file=README.md,line=1::e2e warning annotation"
          echo "### E2E job summary" >> "$GITHUB_STEP_SUMMARY"
      - name: Use secrets
        run: echo "secret=\${{ secrets.E2E_SECRET }} var=\${{ vars.E2E_VAR }}"
      - run: mkdir -p dist && echo "artifact-content" > dist/a.txt
      - uses: actions/upload-artifact@v4
        with:
          name: dist
          path: dist/
      - uses: actions/cache@v4
        with:
          path: cached
          key: e2e-cache-\${{ github.run_id }}-\${{ github.run_attempt }}
      - run: mkdir -p cached && echo x > cached/f
  test:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - run: echo "tests ran"
`;

const DEPLOY = `name: Deploy
on: workflow_dispatch
jobs:
  ship:
    runs-on: ubuntu-latest
    environment: prod
    steps:
      - run: echo "deploying with \${{ secrets.PROD_SECRET }}"
`;

const SLOW = `name: Slow
on: workflow_dispatch
jobs:
  wait:
    runs-on: ubuntu-latest
    steps:
      - run: sleep 120
`;

function forgeBin() {
  const c = [process.env.FORGE_BIN, join(ROOT, "target/debug/forge"), join(ROOT, "data/web-actions/forge")].filter(Boolean);
  const b = c.find((p) => existsSync(p));
  if (!b) throw new Error(`no forge binary (tried ${c.join(", ")}); set FORGE_BIN`);
  return b;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function run(t) {
  const { step, goto, waitText, waitBody, click, fill, clickText, assert, log, shot, git, RUN, WORK, BASE } = t;
  const page = () => t.page;
  const me = t.me;
  const REPO = `act-${RUN}`;
  const full = `${me}/${REPO}`;
  const rp = `/repos/${full}`;
  const runnerName = `e2e-runner-${RUN}`;
  let runner = null;
  let runnerLog = "";

  const waitFor = async (what, fn, timeout = 90000, every = 1000) => {
    const end = Date.now() + timeout;
    let last;
    while (Date.now() < end) {
      last = await fn();
      if (last) return last;
      await sleep(every);
    }
    throw new Error(`timed out waiting for ${what}`);
  };
  const latestRun = (q) => waitFor(`run ${q}`, async () => (await t.rest(`${rp}/actions/runs?${q}`)).json?.workflow_runs?.[0]);
  const runDone = (id, attempt = 1) => waitFor(`run ${id} attempt ${attempt} completed`, async () => {
    const r = (await t.rest(`${rp}/actions/runs/${id}`)).json;
    return r && r.status === "completed" && r.run_attempt === attempt ? r : null;
  }, 150000);
  /** Wait until the run page shows `label` as the run status (the page polls on its own). */
  const uiStatus = (label, timeout = 150000) =>
    page().waitForFunction((l) => document.querySelector('[data-testid="run-status"]')?.textContent === l, { timeout, polling: 500 }, label);
  const openStep = async (name) => {
    const h = await page().waitForSelector(`.act-step-head ::-p-text(${name})`);
    const expanded = await h.evaluate((el) => el.closest("button").getAttribute("aria-expanded"));
    if (expanded !== "true") await h.click();
  };

  const clone = join(WORK, `act-clone-${RUN}`);
  const pushFile = (path, content, msg) => {
    mkdirSync(dirname(join(clone, path)), { recursive: true });
    writeFileSync(join(clone, path), content);
    git(clone, "add", "-A");
    git(clone, "commit", "-q", "-m", msg);
    git(clone, "push", "-q", "origin", "HEAD");
  };

  try {
    await step("actions: empty Actions tab", async () => {
      await t.newRepo(REPO);
      await goto(`/${full}`);
      await click('[data-testid="repo-tab-actions"]');
      await waitText("Get started with GitHub Actions");
      await shot("actions-01-empty");
    });

    await step("actions: new self-hosted runner page and runner comes online", async () => {
      await goto(`/${full}/settings`);
      await click('[data-testid="settings-nav-actions"]');
      await waitText("Actions permissions");
      await clickText("Runners", "a");
      await waitText("There are no runners configured");
      await click('[data-testid="new-runner"]');
      await waitText("Create self-hosted runner");
      const el = await page().waitForSelector('[data-testid="registration-token"]');
      const token = await el.evaluate((e) => e.textContent);
      assert(/^[A-Z0-9]{20,}$/.test(token), `registration token ${token}`);
      const cmd = await page().$eval('input[aria-label="Runner command"]', (e) => e.value);
      assert(cmd.includes(`--token ${token}`) && cmd.startsWith("./forge runner"), cmd);
      await shot("actions-02-new-runner");
      const work = join(WORK, `runner-${RUN}`);
      rmSync(work, { recursive: true, force: true });
      runner = spawn(forgeBin(), ["runner", "--url", BASE, "--token", token, "--name", runnerName, "--work-dir", work, "--config", join(WORK, `runner-${RUN}.json`), "--slots", "2"], {
        env: { ...process.env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null", GIT_TERMINAL_PROMPT: "0" },
        stdio: ["ignore", "pipe", "pipe"],
      });
      runner.stdout.on("data", (d) => (runnerLog += d));
      runner.stderr.on("data", (d) => (runnerLog += d));
      await waitFor("runner online", async () => (await t.rest(`${rp}/actions/runners`)).json?.runners?.some((r) => r.name === runnerName && r.status === "online"), 60000);
      await goto(`/${full}/settings/actions/runners`);
      await page().waitForSelector(`[data-testid="runner-${runnerName}"]`);
      await waitText("Idle");
      await shot("actions-03-runners");
    });

    await step("actions: runner labels (add in the UI)", async () => {
      await click(`[data-testid="runner-add-label-${runnerName}"]`);
      await fill('[data-testid="runner-label-input"]', "e2e-label");
      await page().keyboard.press("Enter");
      await page().waitForSelector('button[aria-label="Remove label e2e-label"]');
      const rs = (await t.rest(`${rp}/actions/runners`)).json.runners.find((r) => r.name === runnerName);
      assert(rs.labels.some((l) => l.name === "e2e-label" && l.type === "custom"), JSON.stringify(rs.labels));
    });

    await step("actions: repository secret and variable created in the UI", async () => {
      await goto(`/${full}/settings`);
      await click('[data-testid="settings-nav-secrets-actions"]');
      await waitText("Repository secrets");
      await click('[data-testid="new-repo-secret"]');
      await fill("#secret-name", "E2E_SECRET");
      await fill("#secret-value", `s3cr3t-${RUN}`);
      await click('[data-testid="secret-submit"]');
      await page().waitForSelector('[data-testid="secret-E2E_SECRET"]');
      // a throwaway secret: create and remove
      await click('[data-testid="new-repo-secret"]');
      await fill("#secret-name", "DOOMED");
      await fill("#secret-value", "x");
      await click('[data-testid="secret-submit"]');
      await click('[data-testid="delete-secret-DOOMED"]');
      await page().waitForFunction(() => !document.querySelector('[data-testid="secret-DOOMED"]'));
      await shot("actions-04-secrets");
      await clickText("Variables", "a");
      await waitText("Repository variables");
      await click('[data-testid="new-repo-variable"]');
      await fill("#var-name", "E2E_VAR");
      await fill("#var-value", "first");
      await click('[data-testid="variable-submit"]');
      await page().waitForSelector('[data-testid="variable-E2E_VAR"]');
      await click('a[aria-label="Edit E2E_VAR"]');
      await page().waitForFunction(() => document.querySelector("#var-value")?.value === "first");
      await fill("#var-value", `hello-var-${RUN}`);
      await click('[data-testid="variable-submit"]');
      await page().waitForFunction((v) => document.querySelector('[data-testid="variable-E2E_VAR"]')?.textContent.includes(v), {}, `hello-var-${RUN}`);
      const v = await t.rest(`${rp}/actions/variables/E2E_VAR`);
      assert(v.json?.value === `hello-var-${RUN}`, `variable ${v.status} ${v.text}`);
      const s = await t.rest(`${rp}/actions/secrets/E2E_SECRET`);
      assert(s.status === 200, `secret ${s.status}`);
      await shot("actions-05-variables");
    });

    let pushRun;
    await step("actions: push a workflow, watch the run finish in the UI", async () => {
      const url = new URL(`${BASE}/${full}.git`);
      url.username = me;
      url.password = t.token;
      git(WORK, "clone", "-q", url.toString(), clone);
      git(clone, "config", "user.name", "E2E Bot");
      git(clone, "config", "user.email", "e2e@example.com");
      pushFile(".github/workflows/deploy.yml", DEPLOY, "Add deploy workflow");
      pushFile(".github/workflows/slow.yml", SLOW, "Add slow workflow");
      pushFile(".github/workflows/ci.yml", CI, "Add CI workflow");
      pushRun = await latestRun("event=push");
      await goto(`/${full}/actions`);
      await page().waitForSelector(`[data-testid="run-${pushRun.id}"]`);
      await page().waitForSelector('[data-testid="wf-ci.yml"]');
      await shot("actions-06-runs");
      await click(`[data-testid="run-${pushRun.id}"] a.act-run-title`);
      await page().waitForSelector('[data-testid="job-graph"]');
      await uiStatus("Success");
      await waitText("test");
      await shot("actions-07-run-summary");
    });

    await step("actions: run summary shows annotations, job summary and artifacts", async () => {
      await page().waitForSelector('[data-testid="annotations"]');
      await waitText("e2e warning annotation");
      await page().waitForSelector('[data-testid="job-summary-build"]');
      await waitText("E2E job summary");
      const href = await page().$eval('[data-testid="artifact-dist"]', (a) => a.getAttribute("href"));
      const dl = await t.pageFetch(href);
      assert(dl.status === 200 && dl.text.startsWith("PK"), `artifact download ${dl.status} ${dl.ct}`);
      await click('[data-testid="run-options"]');
      const logs = await page().$eval('[data-testid="download-run-logs"]', (a) => a.getAttribute("href"));
      await page().keyboard.press("Escape");
      const lz = await t.pageFetch(logs);
      assert(lz.status === 200 && lz.text.startsWith("PK"), `run logs ${lz.status}`);
    });

    await step("actions: job logs with steps (secret masked, variable visible)", async () => {
      await click('[data-testid="job-link-build"]');
      await page().waitForSelector('[data-testid="job-view"]');
      await openStep("Use secrets");
      await waitBody(new RegExp(`secret=\\*\\*\\* var=hello-var-${RUN}`));
      const text = await t.bodyText();
      assert(!text.includes(`s3cr3t-${RUN}`), "secret value leaked into the log view");
      await openStep("Greet");
      await waitText("hello world level=");
      await fill('input[aria-label="Search logs"]', "artifact-content");
      await sleep(300);
      await shot("actions-08-job-logs");
      await click('button[aria-label="Job options"]');
      const raw = await page().$eval('[data-testid="raw-logs"]', (a) => a.getAttribute("href"));
      await page().keyboard.press("Escape");
      const r = await t.pageFetch(raw);
      assert(r.status === 200 && r.text.includes("hello world") && !r.text.includes(`s3cr3t-${RUN}`), `raw logs ${r.status}`);
    });

    await step("actions: delete an artifact", async () => {
      await goto(`/${full}/actions/runs/${pushRun.id}`);
      await page().waitForSelector('[data-testid="delete-artifact-dist"]');
      await click('[data-testid="delete-artifact-dist"]');
      await waitText("No artifacts were produced by this run.");
    });

    await step("actions: re-run all jobs (attempt 2)", async () => {
      await click('[data-testid="run-summary-link"]');
      await click('[data-testid="rerun-menu"]');
      await click('[data-testid="rerun-all"]');
      await waitText("Re-run of all jobs requested.");
      await runDone(pushRun.id, 2);
      await page().waitForSelector('[data-testid="attempt-menu"]', { timeout: 30000 });
      await uiStatus("Success");
      await click('[data-testid="attempt-menu"]');
      await clickText("Attempt #1", "a");
      await page().waitForFunction(() => location.pathname.endsWith("/attempts/1"));
      await uiStatus("Success");
      await shot("actions-09-attempt-1");
    });

    let dispatchRun;
    await step("actions: dispatch the workflow with inputs", async () => {
      await goto(`/${full}/actions`);
      await click('[data-testid="wf-ci.yml"]');
      await waitText("This workflow has a");
      await click('[data-testid="run-workflow"]');
      await page().waitForSelector('[data-testid="dispatch-form"]');
      await fill('[data-testid="input-greeting"]', "from-ui");
      await page().select('[data-testid="input-level"]', "high");
      await click('[data-testid="input-loud"]');
      await shot("actions-10-dispatch-form");
      await click('[data-testid="dispatch-submit"]');
      await waitText("Workflow run was successfully requested.");
      dispatchRun = await latestRun("event=workflow_dispatch");
      await page().waitForSelector(`[data-testid="run-${dispatchRun.id}"]`);
      await click(`[data-testid="run-${dispatchRun.id}"] a.act-run-title`);
      await uiStatus("Success");
      await click('[data-testid="job-link-build"]');
      await openStep("Greet");
      await waitText("hello from-ui level=high loud=true");
    });

    await step("actions: filter runs by event, status, branch and actor", async () => {
      await goto(`/${full}/actions`);
      await page().select('[data-testid="filter-event"]', "workflow_dispatch");
      await page().waitForFunction((id) => !document.querySelector(`[data-testid="run-${id}"]`), {}, pushRun.id);
      await page().waitForSelector(`[data-testid="run-${dispatchRun.id}"]`);
      await page().select('[data-testid="filter-status"]', "failure");
      await waitText("No workflow runs match your filters");
      await clickText("Clear filters");
      await page().waitForSelector(`[data-testid="run-${pushRun.id}"]`);
      await fill('[data-testid="filter-actor"]', me);
      await page().keyboard.press("Enter");
      await page().waitForFunction(() => location.search.includes("actor="));
      await page().waitForSelector(`[data-testid="run-${pushRun.id}"]`);
      await fill('[data-testid="filter-branch"]', "no-such-branch");
      await page().keyboard.press("Enter");
      await waitText("No workflow runs match your filters");
      await shot("actions-11-filters");
    });

    await step("actions: disable and enable a workflow", async () => {
      await goto(`/${full}/actions/workflows/ci.yml`);
      await click('[data-testid="workflow-menu"]');
      await click('[data-testid="workflow-toggle"]');
      await waitText("This workflow was disabled manually.");
      const w = await t.rest(`${rp}/actions/workflows/ci.yml`);
      assert(w.json.state === "disabled_manually", w.text);
      await click('[data-testid="workflow-menu"]');
      await click('[data-testid="workflow-toggle"]');
      await waitText("Workflow enabled successfully.");
      assert((await t.rest(`${rp}/actions/workflows/ci.yml`)).json.state === "active", "enabled again");
    });

    await step("actions: cancel a running workflow and delete the run", async () => {
      await goto(`/${full}/actions/workflows/slow.yml`);
      await click('[data-testid="run-workflow"]');
      await click('[data-testid="dispatch-submit"]');
      await waitText("Workflow run was successfully requested.");
      const slow = await waitFor("slow run", async () => (await t.rest(`${rp}/actions/workflows/slow.yml/runs`)).json?.workflow_runs?.[0]);
      await goto(`/${full}/actions/runs/${slow.id}`);
      await uiStatus("In progress", 60000);
      await click('[data-testid="cancel-run"]');
      await uiStatus("Cancelled", 60000);
      await goto(`/${full}/actions/workflows/slow.yml`);
      await click(`[data-testid="run-menu-${slow.id}"]`);
      await clickText("Delete workflow run");
      await page().waitForFunction((id) => !document.querySelector(`[data-testid="run-${id}"]`), {}, slow.id);
      assert((await t.rest(`${rp}/actions/runs/${slow.id}`)).status === 404, "run deleted");
    });

    await step("actions: caches page lists and deletes caches", async () => {
      await goto(`/${full}/actions`);
      await clickText("Caches", "a");
      await page().waitForSelector('[data-testid="caches-list"]');
      const before = (await t.rest(`${rp}/actions/caches`)).json.total_count;
      assert(before >= 1, `caches ${before}`);
      await shot("actions-12-caches");
      await page().locator('button[aria-label^="Delete cache e2e-cache-"]').click();
      await waitFor("cache deleted", async () => (await t.rest(`${rp}/actions/caches`)).json.total_count === before - 1, 15000);
    });

    let env;
    await step("actions: environment with required reviewer, branch rule and secret", async () => {
      await goto(`/${full}/settings`);
      await click('[data-testid="settings-nav-environments"]');
      await click('[data-testid="new-environment"]');
      await fill("#env-name", "prod");
      await click('[data-testid="configure-environment"]');
      await page().waitForSelector('[data-testid="environment-form"]');
      await click('[data-testid="env-required-reviewers"]');
      await fill('[data-testid="env-reviewer-input"]', me);
      await click('[data-testid="env-add-reviewer"]');
      await page().waitForSelector(`button[aria-label="Remove ${me}"]`);
      await page().select('[data-testid="env-branch-mode"]', "custom");
      await click('[data-testid="env-save"]');
      await waitText("Environment protection rules saved.");
      await page().waitForSelector('[data-testid="branch-policies"]');
      await fill('[data-testid="branch-policy-input"]', "main");
      await click('[data-testid="branch-policy-add"]');
      await page().waitForSelector('button[aria-label="Remove rule main"]');
      await click('[data-testid="new-env-secret"]');
      await fill("#secret-name", "PROD_SECRET");
      await fill("#secret-value", `prod-${RUN}`);
      await click('[data-testid="secret-submit"]');
      await page().waitForSelector('[data-testid="secret-PROD_SECRET"]');
      await click('[data-testid="new-env-variable"]');
      await fill("#var-name", "REGION");
      await fill("#var-value", "eu");
      await click('[data-testid="variable-submit"]');
      await page().waitForSelector('[data-testid="variable-REGION"]');
      await shot("actions-13-environment");
      env = (await t.rest(`${rp}/environments/prod`)).json;
      assert(env.protection_rules.some((r) => r.type === "required_reviewers" && r.reviewers[0].reviewer.login === me), JSON.stringify(env.protection_rules));
      assert(env.deployment_branch_policy?.custom_branch_policies === true, "branch policy");
      await goto(`/${full}/settings/environments`);
      await waitText("1 required reviewer");
    });

    let deployRun;
    await step("actions: approve a pending deployment in the UI", async () => {
      await goto(`/${full}/actions/workflows/deploy.yml`);
      await click('[data-testid="run-workflow"]');
      await click('[data-testid="dispatch-submit"]');
      await waitText("Workflow run was successfully requested.");
      deployRun = await waitFor("deploy run", async () => (await t.rest(`${rp}/actions/workflows/deploy.yml/runs`)).json?.workflow_runs?.[0]);
      await goto(`/${full}/actions/runs/${deployRun.id}`);
      await uiStatus("Waiting", 60000);
      await waitText("prod needs review");
      await shot("actions-14-pending-review");
      await click('[data-testid="review-deployments"]');
      await fill("#review-comment", "ship it");
      await click('[data-testid="approve-deployments"]');
      await uiStatus("Success");
      await click('[data-testid="job-link-ship"]');
      await openStep("deploying");
      await waitText("deploying with ***");
      const appr = (await t.rest(`${rp}/actions/runs/${deployRun.id}/approvals`)).json;
      assert(appr[0]?.state === "approved" && appr[0]?.comment === "ship it", JSON.stringify(appr));
    });

    await step("actions: deployments in the repo sidebar and deployments page", async () => {
      await goto(`/${full}`);
      await page().waitForSelector('[data-testid="deployments-sidebar"]');
      await clickText("Deployments", "a");
      await page().waitForSelector('[data-testid="deployments-list"]');
      await waitText("prod");
      await shot("actions-15-deployments");
    });

    let pr;
    await step("actions: checks and statuses on a pull request", async () => {
      git(clone, "checkout", "-q", "-b", `feature-${RUN}`);
      pushFile("feature.txt", "feature\n", "Add feature");
      const head = git(clone, "rev-parse", "HEAD");
      const r = await t.rest(`${rp}/pulls`, { method: "POST", body: { title: `Checks PR ${RUN}`, head: `feature-${RUN}`, base: "main" } });
      assert(r.status === 201, `create PR ${r.status} ${r.text}`);
      pr = r.json;
      const prRun = await latestRun("event=pull_request");
      await runDone(prRun.id, 1);
      const st = await t.rest(`${rp}/statuses/${head}`, { method: "POST", body: { state: "success", context: "e2e/external", description: "External check passed", target_url: `${BASE}/${full}` } });
      assert(st.status === 201, `status ${st.status}`);
      await goto(`/${full}/pull/${pr.number}`);
      await page().waitForSelector('[data-testid="pr-checks"]');
      await waitText("All checks have passed");
      await clickText("Show all checks");
      await waitText("e2e/external");
      await shot("actions-16-pr-checks");
      await click('[data-testid="pr-checks-tab"]');
      await page().waitForSelector('[data-testid="check-detail"]');
      await click('[data-testid="check-build"]');
      await waitText("e2e warning annotation");
      await waitText("E2E job summary");
      await shot("actions-17-pr-checks-page");
      await goto(`/${full}/commit/${head}`);
      await page().waitForSelector('[data-testid="commit-checks"]');
      await click('[data-testid="commit-checks"] button');
      await waitText("e2e/external");
    });

    await step("actions: Actions general settings (permissions, retention, OIDC)", async () => {
      await goto(`/${full}/settings/actions`);
      await page().waitForSelector('[data-testid="policy-workflow"]');
      await click('[data-testid="wfperm-write"]');
      await click('[data-testid="policy-workflow-save"]');
      await page().waitForSelector('[data-testid="policy-workflow"] .flash-success');
      assert((await t.rest(`${rp}/actions/permissions/workflow`)).json.default_workflow_permissions === "write", "workflow permissions saved");
      await fill('[data-testid="retention-days"]', "30");
      await click('[data-testid="policy-retention-save"]');
      await page().waitForSelector('[data-testid="policy-retention"] .flash-success');
      assert((await t.rest(`${rp}/actions/permissions/artifact-and-log-retention`)).json.days === 30, "retention saved");
      await click('[data-testid="approval-all_external_contributors"]');
      await click('[data-testid="policy-approval-save"]');
      await page().waitForSelector('[data-testid="policy-approval"] .flash-success');
      assert((await t.rest(`${rp}/actions/permissions/fork-pr-contributor-approval`)).json.approval_policy === "all_external_contributors", "approval saved");
      await click('[data-testid="allowed-local"]');
      await click('[data-testid="policy-permissions-save"]');
      await page().waitForSelector('[data-testid="policy-permissions"] .flash-success');
      assert((await t.rest(`${rp}/actions/permissions`)).json.allowed_actions === "local_only", "allowed actions saved");
      // back to all actions so later workflows keep running
      await page().locator('[data-testid="policy-permissions"] input[name="allowed"]').click();
      await click('[data-testid="policy-permissions-save"]');
      await waitFor("allowed all", async () => (await t.rest(`${rp}/actions/permissions`)).json.allowed_actions === "all", 10000);
      await click('[data-testid="oidc-custom"]');
      await fill('[data-testid="oidc-keys"]', "repo, context");
      await click('[data-testid="policy-oidc-save"]');
      await page().waitForSelector('[data-testid="policy-oidc"] .flash-success');
      const o = (await t.rest(`${rp}/actions/oidc/customization/sub`)).json;
      assert(o.use_default === false && o.include_claim_keys.join() === "repo,context", JSON.stringify(o));
      await shot("actions-18-settings");
    });

    await step("actions: other users see runs but no admin controls", async () => {
      const p2 = await t.page2();
      await p2.goto(`${BASE}/${full}/actions/runs/${pushRun.id}`, { waitUntil: "networkidle0" });
      await p2.waitForSelector('[data-testid="run-status"]');
      assert(!(await p2.$('[data-testid="rerun-menu"]')), "no re-run for read-only users");
      assert(!(await p2.$('[data-testid="delete-artifact-dist"]')), "no artifact delete");
      await p2.goto(`${BASE}/${full}/settings/secrets/actions`, { waitUntil: "networkidle0" });
      assert(!(await p2.$('[data-testid="repo-secrets"]')), "secrets settings hidden");
    });

    await step("actions: organization Actions settings (runner groups, org secrets and variables)", async () => {
      const org = `actorg-${RUN}`;
      const c = await t.pageFetch("/api/forge/orgs", { method: "POST", headers: { "Content-Type": "application/json", "X-Forge-CSRF": "1" }, body: JSON.stringify({ login: org }) });
      assert(c.status === 201, `create org ${c.status} ${c.text}`);
      const orepo = await t.rest(`/orgs/${org}/repos`, { method: "POST", body: { name: "app", auto_init: true } });
      assert(orepo.status === 201, `org repo ${orepo.status}`);
      await goto(`/organizations/${org}/settings/actions`);
      await sleep(500);
      if (!(await page().$(".settings-nav"))) {
        log("    SKIP organization settings: the org settings shell (orgs area) is not present");
        return;
      }
      await page().waitForSelector('[data-testid="policy-permissions"]');
      await clickText("Runner groups", "a");
      await page().waitForSelector('[data-testid="runner-groups"]');
      await click('[data-testid="new-runner-group"]');
      await fill("#rg-name", "e2e-group");
      await click('[data-testid="runner-group-save"]');
      await page().waitForSelector("::-p-text(e2e-group)");
      await shot("actions-19-org-runner-groups");
      await goto(`/organizations/${org}/settings/secrets/actions`);
      await click('[data-testid="new-org-secret"]');
      await fill("#secret-name", "ORG_SECRET");
      await fill("#secret-value", "org-value");
      await click('[data-testid="vis-selected"]');
      await page().waitForSelector('[data-testid="repo-picker"]');
      await page().locator(`[data-testid="repo-picker"] input[type="checkbox"]`).click();
      await click('[data-testid="secret-submit"]');
      await page().waitForSelector('[data-testid="secret-ORG_SECRET"]');
      const repos = (await t.rest(`/orgs/${org}/actions/secrets/ORG_SECRET/repositories`)).json;
      assert(repos.total_count === 1 && repos.repositories[0].name === "app", JSON.stringify(repos));
      await goto(`/organizations/${org}/settings/variables/actions`);
      await click('[data-testid="new-org-variable"]');
      await fill("#var-name", "ORG_VAR");
      await fill("#var-value", "org-var");
      await click('[data-testid="variable-submit"]');
      await page().waitForSelector('[data-testid="variable-ORG_VAR"]');
      await goto(`/${org}/app/settings/secrets/actions`);
      await waitText("Organization secrets");
      await waitText("ORG_SECRET");
      await shot("actions-20-org-secrets");
    });

    await step("actions: site admin Actions policies and runner groups", async () => {
      const u = (await t.rest("/user")).json;
      if (!u.site_admin) {
        log("    SKIP site admin: the e2e user is not a site administrator");
        return;
      }
      await goto("/site-admin/actions/runner-groups");
      await sleep(500);
      if (!(await page().$(".settings-nav"))) {
        log("    SKIP site admin: the site admin shell (admin area) is not present");
        return;
      }
      await page().waitForSelector('[data-testid="runner-groups"]');
      await click('[data-testid="new-runner-group"]');
      await fill("#rg-name", `ent-group-${RUN}`);
      await click('[data-testid="runner-group-save"]');
      await page().waitForSelector(`::-p-text(ent-group-${RUN})`);
      await clickText(`ent-group-${RUN}`, "a");
      await click('[data-testid="runner-group-delete"]');
      await page().waitForSelector('[data-testid="runner-groups"]');
      await page().waitForFunction((n) => !document.body.innerText.includes(n), {}, `ent-group-${RUN}`);
      await clickText("Runners", "a");
      await waitText("New self-hosted runner");
      await clickText("General", "a");
      await page().waitForSelector('[data-testid="policy-permissions"]');
      await shot("actions-21-site-admin");
    });

    await step("actions: 375px pages have no horizontal overflow", async () => {
      const m = await page().browserContext().newPage();
      m.setDefaultNavigationTimeout(60000);
      await m.setViewport({ width: 375, height: 800, isMobile: true, hasTouch: true });
      const pages = [
        ["actions-m-runs", `/${full}/actions`],
        ["actions-m-workflow", `/${full}/actions/workflows/ci.yml`],
        ["actions-m-run", `/${full}/actions/runs/${pushRun.id}`],
        ["actions-m-job", `/${full}/actions/runs/${dispatchRun.id}/job/${(await t.rest(`${rp}/actions/runs/${dispatchRun.id}/jobs`)).json.jobs[0].id}`],
        ["actions-m-caches", `/${full}/actions/caches`],
        ["actions-m-pr-checks", `/${full}/pull/${pr.number}/checks`],
        ["actions-m-deployments", `/${full}/deployments`],
        ["actions-m-secrets", `/${full}/settings/secrets/actions`],
        ["actions-m-variables", `/${full}/settings/variables/actions`],
        ["actions-m-environment", `/${full}/settings/environments/${env.id}/edit`],
        ["actions-m-runners", `/${full}/settings/actions/runners`],
        ["actions-m-settings", `/${full}/settings/actions`],
      ];
      for (const [name, p] of pages) {
        await m.goto(BASE + p, { waitUntil: "networkidle0" });
        await sleep(300);
        const over = await m.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
        await shot(name, m);
        assert(over <= 1, `${p} overflows by ${over}px`);
      }
      await m.close();
    });

    await step("actions: remove the runner in the UI", async () => {
      runner?.kill("SIGTERM");
      runner = null;
      await goto(`/${full}/settings/actions/runners`);
      await click(`[data-testid="runner-menu-${runnerName}"]`);
      await click(`[data-testid="runner-remove-${runnerName}"]`);
      await waitText("There are no runners configured");
    });

    await step("actions: delete a workflow run from the run page", async () => {
      await goto(`/${full}/actions/runs/${dispatchRun.id}`);
      await click('[data-testid="run-options"]');
      await click('[data-testid="delete-run"]');
      await page().waitForFunction(() => /\/actions$/.test(location.pathname));
      assert((await t.rest(`${rp}/actions/runs/${dispatchRun.id}`)).status === 404, "deleted");
    });
  } finally {
    if (runner) runner.kill("SIGTERM");
    if (process.env.E2E_RUNNER_LOG) log(runnerLog);
  }
}

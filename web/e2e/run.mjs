#!/usr/bin/env node
// End-to-end test of the forge web UI against a running forge server, in headless Chrome.
//
//   node e2e/run.mjs --base http://localhost:3109 --login alice --password 'secret-pass-1' \
//       [--login2 bob] [--chrome /path/to/chrome] [--headful] [--keep-going]
//
// The logins should be fresh (they are signed up through the UI; if they already exist the
// script signs in instead). Needs `git` and `ssh-keygen` on PATH. Every git call runs with
// GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 so no credential helper
// or user config is touched. Screenshots go to e2e/screenshots/. Exit status 0 = all passed.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

// ---------- args ----------
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith("--")) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith("--") ? all[i + 1] : true]);
    return acc;
  }, []),
);
const BASE = String(args.base || "http://localhost:3109").replace(/\/$/, "");
const LOGIN = args.login;
const PASSWORD = args.password;
if (!LOGIN || !PASSWORD) {
  console.error("usage: node e2e/run.mjs --base URL --login NAME --password PASS [--login2 NAME] [--chrome PATH] [--headful]");
  process.exit(2);
}
const LOGIN2 = args.login2 || `${LOGIN}-b`;
const CHROME = args.chrome || process.env.CHROME_PATH || [
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
].find((p) => existsSync(p));
const HERE = dirname(fileURLToPath(import.meta.url));
const SHOTS = join(HERE, "screenshots");
mkdirSync(SHOTS, { recursive: true });
const WORK = mkdtempSync(join(tmpdir(), "forge-e2e-"));
const RUN = Date.now().toString(36).slice(-5);
const REPO = `demo-${RUN}`;
const EMPTY = `empty-${RUN}`;
const DOOMED = `doomed-${RUN}`;

// ---------- helpers ----------
const results = [];
let page;
let browser;

function log(...m) {
  console.log(...m);
}

function assert(cond, msg) {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
}

const GIT_ENV = {
  ...process.env,
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_TERMINAL_PROMPT: "0",
  GIT_AUTHOR_NAME: "E2E Bot",
  GIT_AUTHOR_EMAIL: "e2e@example.com",
  GIT_COMMITTER_NAME: "E2E Bot",
  GIT_COMMITTER_EMAIL: "e2e@example.com",
};

function git(cwd, ...a) {
  const r = spawnSync("git", a, { cwd, env: GIT_ENV, encoding: "utf8" });
  if (r.status !== 0) throw new Error(`git ${a.join(" ")} failed:\n${r.stdout}\n${r.stderr}`);
  return r.stdout.trim();
}

async function shot(name, p = page) {
  await p.screenshot({ path: join(SHOTS, `${name}.png`), fullPage: true });
}

async function step(name, fn) {
  const t = Date.now();
  try {
    await fn();
    results.push({ name, ok: true });
    log(`  ok   ${name} (${Date.now() - t} ms)`);
  } catch (e) {
    results.push({ name, ok: false, err: e });
    log(`  FAIL ${name}: ${e.message}`);
    try {
      await shot(`FAIL-${name.replace(/[^a-z0-9]+/gi, "-")}`);
    } catch {
      /* ignore */
    }
    if (!args["keep-going"]) throw e;
  }
}

async function goto(path, p = page) {
  await p.goto(BASE + path, { waitUntil: "networkidle0" });
}

/** Wait until some element contains `text` (Puppeteer text selector). */
async function waitText(text, p = page, timeout = 30000) {
  await p.waitForSelector(`::-p-text(${text})`, { timeout });
}

async function bodyText(p = page) {
  return p.evaluate(() => document.body.innerText);
}

async function waitBody(re, p = page, timeout = 30000) {
  const src = re instanceof RegExp ? re.source : re.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const flags = re instanceof RegExp ? re.flags : "";
  await p.waitForFunction((s, f) => new RegExp(s, f).test(document.body.innerText), { timeout }, src, flags);
}

async function click(selector, p = page) {
  await p.locator(selector).click();
}

async function fill(selector, value, p = page) {
  await p.locator(selector).fill(value);
}

/** Click a button/link by its visible text. */
async function clickText(text, tag = "button", p = page) {
  await p.locator(`${tag}::-p-text(${text})`).click();
}

/** fetch() from inside the page so the session cookie is used. */
async function pageFetch(path, init = {}, p = page) {
  return p.evaluate(
    async (u, i) => {
      const r = await fetch(u, { credentials: "same-origin", cache: "no-store", ...i });
      const ct = r.headers.get("content-type") || "";
      return { status: r.status, ct, text: await r.text() };
    },
    path,
    init,
  );
}

async function api(path, init = {}, p = page) {
  const headers = { Accept: "application/vnd.github+json", "X-Forge-CSRF": "1", ...(init.headers || {}) };
  if (init.body && typeof init.body !== "string") {
    init = { ...init, body: JSON.stringify(init.body) };
    headers["Content-Type"] = "application/json";
  }
  const r = await pageFetch(`/api/v3${path}`, { ...init, headers }, p);
  return { ...r, json: r.text ? JSON.parse(r.text) : null };
}

/** Log API requests that take longer than 3 s or never finish (helps diagnose hangs). */
function watchSlow(p) {
  const started = new Map();
  p.on("request", (r) => started.set(r, Date.now()));
  const done = (r, how) => {
    const t = started.get(r);
    started.delete(r);
    if (t && Date.now() - t > 3000) log(`    slow ${how} ${r.method()} ${r.url()} ${Date.now() - t} ms`);
  };
  p.on("requestfinished", (r) => done(r, "ok"));
  p.on("requestfailed", (r) => done(r, `failed(${r.failure()?.errorText})`));
}

async function signUpOrIn(p, login, password) {
  await goto("/signup", p);
  await fill("#su-login", login, p);
  await fill("#su-email", `${login}@example.com`, p);
  await fill("#su-password", password, p);
  await clickText("Create account", "button", p);
  try {
    await p.waitForFunction(() => location.pathname === "/", { timeout: 5000 });
  } catch {
    // Probably "already exists": sign in instead.
    await goto("/login", p);
    await fill("#login", login, p);
    await fill("#password", password, p);
    await clickText("Sign in", "button", p);
    await p.waitForFunction(() => location.pathname === "/", { timeout: 10000 });
  }
  await waitText("Welcome", p);
}

// ---------- the run ----------
async function main() {
  log(`forge e2e: base=${BASE} users=${LOGIN},${LOGIN2} repo=${REPO} chrome=${CHROME}`);
  browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: !args.headful,
    protocolTimeout: 180000,
    args: ["--no-first-run", "--no-default-browser-check", "--disable-features=Translate"],
  });
  const ctx1 = await browser.createBrowserContext();
  page = await ctx1.newPage();
  // Generous timeouts: the forge dev build on a busy machine can take seconds per request.
  page.setDefaultTimeout(45000);
  page.setDefaultNavigationTimeout(60000);
  await page.setViewport({ width: 1280, height: 900 });
  page.on("dialog", (d) => d.accept());
  const consoleErrors = [];
  page.on("pageerror", (e) => consoleErrors.push(String(e)));
  watchSlow(page);

  let token = "";
  let me = LOGIN;
  const full = () => `${me}/${REPO}`;

  await step("landing page (signed out)", async () => {
    await goto("/");
    await waitText("Where code gets forged");
    await shot("01-landing");
  });

  await step("sign up", async () => {
    await signUpOrIn(page, LOGIN, PASSWORD);
    await shot("02-dashboard");
  });

  await step("sign out and sign in", async () => {
    await click('button[aria-label="Open user menu"]');
    await clickText("Sign out", "button");
    await waitText("Where code gets forged");
    const who = await pageFetch("/api/v3/user");
    assert(who.status === 401, `GET /user after logout = ${who.status}`);
    await goto("/login");
    await fill("#login", LOGIN);
    await fill("#password", "definitely-wrong");
    await clickText("Sign in", "button");
    await waitText("Incorrect username or password");
    await fill("#password", PASSWORD);
    await clickText("Sign in", "button");
    await waitText("Welcome");
  });

  await step("create token", async () => {
    await goto("/settings/tokens");
    await fill("#token-name", "e2e");
    await clickText("Generate token", "button");
    await page.waitForSelector('input[aria-label="New token"]');
    token = await page.$eval('input[aria-label="New token"]', (e) => e.value);
    assert(/^gho_[A-Za-z0-9]{36}$/.test(token), `token format: ${token}`);
    await waitText("Never used.");
    await shot("03-tokens");
    // the token works against the API
    const r = await fetch(`${BASE}/api/v3/user`, { headers: { Authorization: `token ${token}` } });
    const u = await r.json();
    assert(r.ok && u.login.toLowerCase() === LOGIN.toLowerCase(), "token authenticates");
    me = u.login;
  });

  await step("create repo with README", async () => {
    await goto("/new");
    await fill("#repo-name", REPO);
    await fill("#repo-desc", "An e2e demo repository");
    await clickText("Create repository", "button");
    await page.waitForFunction((p) => location.pathname === p, {}, `/${me}/${REPO}`);
    await waitText("README.md");
    await page.waitForSelector("#readme .markdown-body h1");
    const h1 = await page.$eval("#readme .markdown-body h1", (e) => e.textContent);
    assert(h1.includes(REPO), `README heading: ${h1}`);
    await waitBody(/An e2e demo repository/);
    await shot("04-repo-home");
  });

  await step("browse blob + raw", async () => {
    await click(`a[href="/${me}/${REPO}/blob/main/README.md"]`);
    await page.waitForSelector('[data-testid="raw-link"]');
    await clickText("Code", "button");
    await page.waitForSelector(".code-table");
    const raw = await pageFetch(`/${me}/${REPO}/raw/main/README.md`);
    assert(raw.status === 200 && raw.text.includes(REPO) && !raw.ct.includes("html"), `raw: ${raw.status} ${raw.ct}`);
    await shot("05-blob");
  });

  // Push several branches over smart HTTP with stock git.
  const clone = join(WORK, "clone");
  await step("git clone + push branches (incl. slashed name)", async () => {
    const url = new URL(`${BASE}/${me}/${REPO}.git`);
    url.username = me;
    url.password = token;
    git(WORK, "clone", "-q", url.toString(), clone);
    git(clone, "checkout", "-q", "-b", "feature/deep/slash");
    mkdirSync(join(clone, "src", "lib"), { recursive: true });
    writeFileSync(join(clone, "src", "lib", "hello.js"), 'export function hello(name) {\n  return "hello " + name; // e2e-needle\n}\n');
    writeFileSync(join(clone, "README.md"), `# ${REPO}\n\nUpdated on a slashed branch.\n`);
    git(clone, "add", "-A");
    git(clone, "commit", "-q", "-m", "Add hello library\n\nLonger body for the commit page.");
    git(clone, "push", "-q", "origin", "feature/deep/slash");
    for (const [b, f] of [["squash-me", "squash.txt"], ["rebase-me", "rebase.txt"]]) {
      git(clone, "checkout", "-q", "main");
      git(clone, "checkout", "-q", "-b", b);
      writeFileSync(join(clone, f), `${b}\n`);
      git(clone, "add", "-A");
      git(clone, "commit", "-q", "-m", `Add ${f}`);
      writeFileSync(join(clone, f), `${b}\nsecond line\n`);
      git(clone, "commit", "-q", "-am", `Extend ${f}`);
      git(clone, "push", "-q", "origin", b);
    }
    git(clone, "checkout", "-q", "main");
  });

  await step("branches page", async () => {
    await goto(`/${me}/${REPO}/branches`);
    await waitText("feature/deep/slash");
    const n = await page.$$eval('[data-testid="branch-row"]', (e) => e.length);
    assert(n === 4, `branch rows: ${n}`);
    await shot("06-branches");
  });

  await step("tree + blob on slashed branch (deep link refresh)", async () => {
    await goto(`/${me}/${REPO}/tree/feature/deep/slash/src`);
    await waitText("lib");
    await page.reload({ waitUntil: "networkidle0" });
    await waitText("lib");
    await click(`a[href="/${me}/${REPO}/tree/feature/deep/slash/src/lib"]`);
    await waitText("hello.js");
    await click(`a[href="/${me}/${REPO}/blob/feature/deep/slash/src/lib/hello.js"]`);
    await page.waitForSelector(".code-table");
    await waitBody(/e2e-needle/);
    const sel = await page.$eval('select[aria-label="Switch branch"]', (e) => e.value);
    assert(sel === "feature/deep/slash", `branch select: ${sel}`);
    const raw = await pageFetch(`/${me}/${REPO}/raw/feature/deep/slash/src/lib/hello.js`);
    assert(raw.status === 200 && raw.text.includes("e2e-needle"), `raw on slashed branch: ${raw.status}`);
    await shot("07-blob-slashed");
  });

  await step("commits list + commit diff", async () => {
    await goto(`/${me}/${REPO}/commits/feature/deep/slash`);
    await waitText("Add hello library");
    await page.reload({ waitUntil: "networkidle0" });
    await clickText("Add hello library", "a");
    await page.waitForSelector(".diff-table");
    await waitBody(/Longer body for the commit page/);
    await waitBody(/src\/lib\/hello\.js/);
    await waitBody(/2 changed files/);
    await shot("08-commit");
  });

  let prMerge = 0;
  await step("compare + open PR (merge commit)", async () => {
    await goto(`/${me}/${REPO}/compare/main...feature/deep/slash`);
    await waitText("1 commit");
    await page.waitForSelector(".diff-table");
    await shot("09-compare");
    await fill("#pr-title", "Add hello library");
    await fill('textarea[id]', "This adds **hello**.\n\nFixes nothing.");
    await clickText("Create pull request", "button");
    await page.waitForFunction(() => /\/pull\/\d+$/.test(location.pathname));
    prMerge = Number((await page.evaluate(() => location.pathname)).split("/").pop());
    await waitText("wants to merge");
    await waitBody(/This branch has no conflicts/);
  });

  await step("PR files tab (deep link refresh)", async () => {
    await goto(`/${me}/${REPO}/pull/${prMerge}/files`);
    await page.waitForSelector(".diff-table");
    await page.reload({ waitUntil: "networkidle0" });
    await page.waitForSelector(".diff-table");
    await waitBody(/src\/lib\/hello\.js/);
    await shot("10-pr-files");
    await clickText("Commits", "a");
    await waitText("Add hello library");
  });

  await step("PR comment + self review (comment)", async () => {
    await goto(`/${me}/${REPO}/pull/${prMerge}`);
    await page.waitForSelector('form[aria-label="Add a comment"] textarea');
    await fill('form[aria-label="Add a comment"] textarea', "Looks good to me, see #1");
    await clickText("Comment", 'form[aria-label="Add a comment"] button[type=submit]');
    await waitText("Looks good to me");
    await click('[data-testid="review-open"]');
    await fill('form[aria-label="Submit review"] textarea', "Self-review note");
    await clickText("Submit review", "button");
    await page.waitForSelector('[data-testid="review-event"]');
    await waitText("Self-review note");
  });

  // Second user: approve, star, fork.
  const ctx2 = await browser.createBrowserContext();
  const page2 = await ctx2.newPage();
  page2.setDefaultTimeout(45000);
  page2.setDefaultNavigationTimeout(60000);
  await page2.setViewport({ width: 1280, height: 900 });
  page2.on("dialog", (d) => d.accept());
  watchSlow(page2);

  await step("second user approves PR", async () => {
    await signUpOrIn(page2, LOGIN2, PASSWORD);
    await goto(`/${me}/${REPO}/pull/${prMerge}`, page2);
    await click('[data-testid="review-open"]', page2);
    await fill('form[aria-label="Submit review"] textarea', "Ship it", page2);
    await page2.evaluate(() => {
      const radios = [...document.querySelectorAll('input[name="review-event"]')];
      radios[1].click();
    });
    await clickText("Submit review", "button", page2);
    await waitText("approved these changes", page2);
    await shot("11-pr-approved", page2);
  });

  await step("merge PR with merge commit", async () => {
    await page.reload({ waitUntil: "networkidle0" });
    await waitText("approved these changes");
    await page.select("#merge-method", "merge");
    await clickText("Merge pull request", "button");
    await clickText("Confirm create a merge commit", "button");
    await waitBody(/merged commit/);
    await page.waitForSelector(".state-merged");
    await shot("12-pr-merged");
  });

  for (const [branch, method, label] of [["squash-me", "squash", "Squash and merge"], ["rebase-me", "rebase", "Rebase and merge"]]) {
    await step(`open + merge PR with ${method}`, async () => {
      await goto(`/${me}/${REPO}/compare/main...${branch}`);
      await waitText("2 commits");
      await clickText("Create pull request", "button");
      await page.waitForFunction(() => /\/pull\/\d+$/.test(location.pathname));
      await waitBody(/This branch has no conflicts/);
      await page.select("#merge-method", method);
      await clickText(label, "button");
      await clickText(`Confirm ${label.toLowerCase()}`, "button");
      await page.waitForSelector(".state-merged");
      const n = Number((await page.evaluate(() => location.pathname)).split("/").pop());
      const pr = await api(`/repos/${me}/${REPO}/pulls/${n}`);
      assert(pr.json.merged === true, `PR ${n} merged`);
      const c = await api(`/repos/${me}/${REPO}/commits/${pr.json.merge_commit_sha}`);
      const parents = c.json.parents.length;
      assert(parents === 1, `${method} produces a single-parent commit (got ${parents})`);
    });
  }

  await step("pulls list shows merged PRs", async () => {
    await goto(`/${me}/${REPO}/pulls?state=closed`);
    await waitText("Add hello library");
    const n = await page.$$eval(".issue-row", (e) => e.length);
    assert(n === 3, `closed PRs: ${n}`);
  });

  let issueNo = 0;
  await step("issue create / comment / label / close / reopen", async () => {
    await goto(`/${me}/${REPO}/issues/new`);
    await fill("#issue-title", "Something is broken");
    await fill(".md-editor textarea", "Steps to reproduce: run it.");
    await clickText("Submit new issue", "button");
    await page.waitForFunction(() => /\/issues\/\d+$/.test(location.pathname));
    issueNo = Number((await page.evaluate(() => location.pathname)).split("/").pop());
    await waitText("Steps to reproduce");
    await fill('form[aria-label="Add a comment"] textarea', "I can reproduce this.");
    await clickText("Comment", 'form[aria-label="Add a comment"] button[type=submit]');
    await waitText("I can reproduce this.");
    // labels
    await clickText("Edit", ".sidebar-heading button");
    await page.waitForSelector(".label-editor input[type=checkbox]");
    await page.evaluate(() => {
      for (const l of document.querySelectorAll(".label-editor .check-row")) {
        if (/^(bug|documentation)$/.test(l.textContent.trim())) l.querySelector("input").click();
      }
    });
    await clickText("Apply", "button");
    await page.waitForSelector(".sidebar-section .label-list .label-pill");
    const labels = await page.$$eval(".sidebar-section .label-list .label-pill", (e) => e.map((x) => x.textContent));
    assert(labels.includes("bug") && labels.includes("documentation"), `labels: ${labels}`);
    await shot("13-issue");
    // close / reopen
    await clickText("Close issue", "button");
    await waitBody(/This issue was closed/);
    await page.waitForSelector(".state-done");
    await clickText("Reopen issue", "button");
    await page.waitForSelector(".issue-meta .state-open");
    // persisted?
    await page.reload({ waitUntil: "networkidle0" });
    await waitText("I can reproduce this.");
    await page.waitForSelector(".issue-meta .state-open");
    // label filter on the list
    await goto(`/${me}/${REPO}/issues?labels=bug`);
    await waitText("Something is broken");
    await shot("14-issues");
  });

  await step("releases: create with asset, view, download", async () => {
    await goto(`/${me}/${REPO}/releases`);
    await waitText("There aren't any releases here");
    await clickText("Draft a new release", "a");
    await fill("#rel-tag", "v1.0.0");
    await fill("#rel-title", "First release");
    await fill(".md-editor textarea", "## Notes\n\n- first!");
    const asset = join(WORK, "forge-e2e-asset.txt");
    writeFileSync(asset, `asset body ${RUN}\n`);
    const input = await page.$("#rel-files");
    await input.uploadFile(asset);
    await clickText("Publish release", "button");
    await page.waitForFunction(() => location.pathname.endsWith("/releases/tag/v1.0.0"), { timeout: 15000 });
    await waitText("First release");
    await waitText("forge-e2e-asset.txt");
    await shot("15-release");
    const href = await page.$eval('[data-testid="asset-link"]', (a) => a.getAttribute("href"));
    assert(href.startsWith(`/${me}/${REPO}/releases/download/`), `asset href ${href}`);
    const dl = await pageFetch(href);
    assert(dl.status === 200 && dl.text === `asset body ${RUN}\n`, `asset download: ${dl.status} ${dl.text}`);
    // upload a second asset from the release page itself
    const asset2 = join(WORK, "second.bin");
    writeFileSync(asset2, Buffer.from([0, 1, 2, 3, 255]));
    await (await page.$('[data-testid="asset-input"]')).uploadFile(asset2);
    await clickText("Upload", "button");
    await waitText("second.bin");
    // list page + latest + refresh
    await goto(`/${me}/${REPO}/releases`);
    await waitText("First release");
    await waitText("Latest");
    await goto(`/${me}/${REPO}/releases/latest`);
    await page.waitForFunction(() => location.pathname.endsWith("/releases/tag/v1.0.0"));
    await page.reload({ waitUntil: "networkidle0" });
    await waitText("First release");
    const zip = await pageFetch(`/${me}/${REPO}/archive/refs/tags/v1.0.0.zip`);
    assert(zip.status === 200 && !zip.ct.includes("html"), `source zip: ${zip.status} ${zip.ct}`);
  });

  await step("tags page", async () => {
    await goto(`/${me}/${REPO}/tags`);
    await waitText("v1.0.0");
    await shot("16-tags");
  });

  await step("stars (both users)", async () => {
    await goto(`/${me}/${REPO}`);
    await page.waitForSelector('[data-testid="star-button"]:not([disabled])');
    await click('[data-testid="star-button"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="star-count"]')?.textContent === "1");
    await waitText("Starred");
    await goto(`/${me}/${REPO}`, page2);
    await page2.waitForSelector('[data-testid="star-button"]:not([disabled])');
    await click('[data-testid="star-button"]', page2);
    await page2.waitForFunction(() => document.querySelector('[data-testid="star-count"]')?.textContent === "2");
    await page.reload({ waitUntil: "networkidle0" });
    await page.waitForFunction(() => document.querySelector('[data-testid="star-count"]')?.textContent === "2");
    // unstar + restar for user 1
    await click('[data-testid="star-button"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="star-count"]')?.textContent === "1");
    await click('[data-testid="star-button"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="star-count"]')?.textContent === "2");
    await goto(`/${me}?tab=stars`);
    await waitText(`${me}/${REPO}`);
  });

  let me2 = LOGIN2;
  await step("fork (second user)", async () => {
    me2 = (await api("/user", {}, page2)).json.login;
    await goto(`/${me}/${REPO}`, page2);
    await page2.waitForSelector('[data-testid="fork-button"]:not([disabled])');
    await click('[data-testid="fork-button"]', page2);
    try {
      await page2.waitForFunction((p) => location.pathname === p, { timeout: 20000 }, `/${me2}/${REPO}`);
    } catch (e) {
      await shot("FAIL-fork-page2", page2);
      throw new Error(`${e.message}; page2 at ${page2.url()}: ${(await bodyText(page2)).slice(0, 300)}`);
    }
    await waitText(`forked from`, page2);
    await waitText("README.md", page2);
    await shot("17-fork", page2);
    await goto(`/${me}/${REPO}`);
    await page.waitForFunction(() => document.querySelector('[data-testid="fork-count"]')?.textContent === "1");
  });

  await step("profile pages", async () => {
    await goto(`/${me}`);
    await waitText(REPO);
    await shot("18-profile");
    await goto(`/${me2}`, page2);
    await waitText(REPO, page2);
    await page2.waitForSelector('[data-testid="fork-note"]');
  });

  await step("search: repositories, issues, code", async () => {
    await goto(`/search?q=${encodeURIComponent(REPO)}&type=repositories`);
    await page.waitForSelector('[data-testid="search-results"]');
    await waitText(`${me}/${REPO}`);
    await goto(`/search?q=${encodeURIComponent(`Something is broken repo:${me}/${REPO}`)}&type=issues`);
    await page.waitForSelector('[data-testid="search-results"]');
    await waitText("Something is broken");
    await goto(`/search?q=${encodeURIComponent(`e2e-needle repo:${me}/${REPO}`)}&type=code`);
    await page.waitForSelector('[data-testid="search-results"]', { timeout: 15000 });
    await waitText("src/lib/hello.js");
    await shot("19-search-code");
    await clickText("src/lib/hello.js", "a");
    await page.waitForSelector(".code-table");
    await waitBody(/e2e-needle/);
    // header search box, scoped to repo
    await goto(`/${me}/${REPO}`);
    await fill("#header-q", "e2e-needle");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => location.pathname === "/search");
    await waitText("src/lib/hello.js");
  });

  await step("SSH keys add / list / delete", async () => {
    const kf = join(WORK, "id_e2e");
    const r = spawnSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-C", "e2e", "-f", kf], { encoding: "utf8" });
    assert(r.status === 0, `ssh-keygen: ${r.stderr}`);
    const pub = readFileSync(kf + ".pub", "utf8").trim();
    await goto("/settings/keys");
    await waitText("There are no SSH keys");
    await fill("#key-title", "e2e laptop");
    await fill("#key-body", pub);
    await clickText("Add SSH key", "button");
    await page.waitForSelector('[data-testid="ssh-key-row"]');
    await waitText("e2e laptop");
    await page.reload({ waitUntil: "networkidle0" });
    await waitText("e2e laptop");
    const dot = await pageFetch(`/${me}.keys`);
    assert(dot.status === 200 && dot.text.includes(pub.split(" ")[1]), `/${me}.keys`);
    // duplicate key is rejected with the server's message
    await fill("#key-title", "dupe");
    await fill("#key-body", pub);
    await clickText("Add SSH key", "button");
    await waitBody(/already in use|already_exists|already exists/i);
    await shot("20-ssh-keys");
    await clickText("Delete", '[data-testid="ssh-key-row"] button');
    await waitText("There are no SSH keys");
  });

  await step("empty repo shows HTTP and SSH clone URLs", async () => {
    await goto("/new");
    await fill("#repo-name", EMPTY);
    await page.evaluate(() => {
      const cb = [...document.querySelectorAll("input[type=checkbox]")].find((c) => c.closest("label")?.textContent?.includes("README"));
      if (cb && cb.checked) cb.click();
    });
    await clickText("Create repository", "button");
    await page.waitForSelector('[data-testid="empty-repo"]');
    const repo = (await api(`/repos/${me}/${EMPTY}`)).json;
    const vals = await page.$$eval('[data-testid="empty-repo"] input', (e) => e.map((x) => x.value));
    assert(vals.includes(repo.clone_url), `https url ${repo.clone_url} in ${vals}`);
    assert(vals.includes(repo.ssh_url), `ssh url ${repo.ssh_url} in ${vals}`);
    await shot("21-empty-repo");
  });

  await step("repo settings: default branch, rename, delete", async () => {
    // default branch
    await goto(`/${me}/${REPO}/settings`);
    await page.waitForSelector("#set-branch");
    await page.select("#set-branch", "feature/deep/slash");
    await clickText("Save changes", "button");
    await waitText("Repository settings saved.");
    let repo = (await api(`/repos/${me}/${REPO}`)).json;
    assert(repo.default_branch === "feature/deep/slash", `default branch ${repo.default_branch}`);
    await page.select("#set-branch", "main");
    const patched = page.waitForResponse((r) => r.request().method() === "PATCH" && r.url().endsWith(`/repos/${me}/${REPO}`));
    await clickText("Save changes", "button");
    await patched;
    repo = (await api(`/repos/${me}/${REPO}`)).json;
    assert(repo.default_branch === "main", `default branch back to main: ${repo.default_branch}`);
    // rename + delete a throwaway repo
    const created = await api("/user/repos", { method: "POST", body: { name: DOOMED, auto_init: true } });
    assert(created.status === 201, `create ${DOOMED}: ${created.status}`);
    await goto(`/${me}/${DOOMED}/settings`);
    await fill("#set-name", `${DOOMED}-renamed`);
    await clickText("Save changes", "button");
    await page.waitForFunction((p) => location.pathname === p, {}, `/${me}/${DOOMED}-renamed/settings`);
    await waitText("Danger Zone");
    await shot("22-settings");
    await fill("#del-confirm", `${me}/${DOOMED}-renamed`);
    await clickText("Delete this repository", "button");
    await page.waitForFunction(() => location.pathname === "/");
    const gone = await api(`/repos/${me}/${DOOMED}-renamed`);
    assert(gone.status === 404, `deleted repo status ${gone.status}`);
  });

  await step("server routes are not shadowed by the SPA", async () => {
    const checks = [
      [`/${me}/${REPO}/info/refs?service=git-upload-pack`, (r) => r.status === 200 && r.ct.includes("git-upload-pack")],
      [`/${me}/${REPO}/raw/main/README.md`, (r) => r.status === 200 && !r.ct.includes("html")],
      [`/${me}/${REPO}/archive/refs/heads/main.tar.gz`, (r) => r.status === 200 && !r.ct.includes("html")],
      [`/${me}.keys`, (r) => r.status === 200 && r.ct.startsWith("text/plain")],
      [`/avatars/${me}`, (r) => r.status === 200 && r.ct.startsWith("image/svg")],
      ["/api/v3/nope", (r) => r.status === 404 && r.ct.includes("json")],
      ["/api/forge/nope", (r) => r.status === 404 && r.ct.includes("json")],
      ["/api/uploads/nope", (r) => r.status === 404 && r.ct.includes("json")],
      [`/${me}/${REPO}/pull/${prMerge}/files`, (r) => r.status === 200 && r.ct.includes("html")],
      [`/${me}/${REPO}/releases/tag/v1.0.0`, (r) => r.status === 200 && r.ct.includes("html")],
    ];
    for (const [p, ok] of checks) {
      const r = await pageFetch(p);
      assert(ok(r), `${p} -> ${r.status} ${r.ct}`);
    }
  });

  await step("dark mode (system preference + toggle)", async () => {
    await page.emulateMediaFeatures([{ name: "prefers-color-scheme", value: "dark" }]);
    await goto(`/${me}/${REPO}/pull/${prMerge}/files`);
    await page.waitForSelector(".diff-table");
    const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    assert(bg === "rgb(13, 17, 23)", `dark body bg ${bg}`);
    await shot("23-dark-pr-files");
    await goto(`/${me}/${REPO}`);
    await shot("24-dark-repo");
    await page.emulateMediaFeatures([{ name: "prefers-color-scheme", value: "light" }]);
    // explicit toggle: auto -> light -> dark
    await click('[data-testid="theme-toggle"]');
    await click('[data-testid="theme-toggle"]');
    const theme = await page.evaluate(() => document.documentElement.dataset.theme);
    assert(theme === "dark", `theme attr ${theme}`);
    const bg2 = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    assert(bg2 === "rgb(13, 17, 23)", `toggled dark bg ${bg2}`);
    await page.reload({ waitUntil: "networkidle0" });
    const kept = await page.evaluate(() => document.documentElement.dataset.theme);
    assert(kept === "dark", "theme persists across reload");
    await click('[data-testid="theme-toggle"]'); // back to auto
  });

  await step("mobile width (375px) has no horizontal overflow", async () => {
    const m = await ctx1.newPage();
    m.setDefaultNavigationTimeout(60000);
    await m.setViewport({ width: 375, height: 800, isMobile: true, hasTouch: true });
    const pages = [
      ["25-mobile-repo", `/${me}/${REPO}`],
      ["26-mobile-pr", `/${me}/${REPO}/pull/${prMerge}`],
      ["27-mobile-issue", `/${me}/${REPO}/issues/${issueNo}`],
      ["28-mobile-release", `/${me}/${REPO}/releases`],
      ["29-mobile-commit-diff", `/${me}/${REPO}/pull/${prMerge}/files`],
      ["30-mobile-search", `/search?q=${REPO}`],
    ];
    for (const [name, p] of pages) {
      await goto(p, m);
      const over = await m.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      await shot(name, m);
      assert(over <= 1, `${p} overflows by ${over}px`);
    }
    await m.close();
  });

  await step("no uncaught page errors", async () => {
    assert(consoleErrors.length === 0, consoleErrors.join("\n"));
  });
}

let code = 0;
try {
  await main();
} catch {
  code = 1;
} finally {
  await browser?.close().catch(() => {});
  rmSync(WORK, { recursive: true, force: true });
}
const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} steps passed; screenshots in ${SHOTS}`);
if (failed.length) {
  for (const f of failed) console.log(`FAILED: ${f.name}\n${f.err?.stack || f.err}`);
  code = 1;
}
process.exit(code);

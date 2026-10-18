// Social area e2e: notifications (bell, inbox, filters, mark read/done, unsubscribe), watching
// (incl. custom, /watching), issue/PR subscriptions, follows, stargazers/watchers, activity feed,
// gists (full lifecycle), user settings (profile, emails, password, GPG + SSH signing keys,
// appearance, blocked users, account: rename, export, delete), explore, 375 px checks.
// Idempotent: reruns with the same logins work (state is reset or tolerated where it persists).
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const { gpgPublicKey } = await import(join(HERE, "../../../tools/conformance/scenarios/write-checks.mjs"));

export async function run(t) {
  const { step, goto, waitText, waitBody, click, fill, clickText, shot, assert, api, RUN, PASSWORD, WORK } = t;
  const page = () => t.page;
  const me = t.me;
  const REPO = `social-${RUN}`;
  const full = `${me}/${REPO}`;
  let p2;
  let other = t.LOGIN2;
  let issueNo = 0;
  let pullNo = 0;
  let gistId = "";
  let publicGistId = "";

  const waitFn = (p, fn, arg, timeout = 30000) => p.waitForFunction(fn, { timeout }, arg);
  const testText = (p, id) => p.$eval(`[data-testid="${id}"]`, (e) => e.textContent.trim());
  /** Poll an async predicate (API state settles asynchronously, e.g. notifications). */
  async function until(fn, msg, timeout = 30000) {
    const end = Date.now() + timeout;
    for (;;) {
      if (await fn()) return;
      if (Date.now() > end) throw new Error(`timed out: ${msg}`);
      await new Promise((r) => setTimeout(r, 500));
    }
  }

  await step("social: second user signs in", async () => {
    p2 = await t.page2();
    const r = await api("/user", {}, p2);
    other = r.json.login;
  });

  await step("social: repo watch button (all / custom / watchers page)", async () => {
    await t.newRepo(REPO, { description: "Social e2e repository" });
    await goto(`/${full}`);
    await page().waitForSelector('[data-testid="watch-button"]');
    await click('[data-testid="watch-button"]');
    await page().waitForSelector('[data-testid="watch-menu"]');
    await shot("social-01-watch-menu");
    await click('[data-testid="watch-option-custom"]');
    await click('[data-testid="watch-custom-issues"]');
    await click('[data-testid="watch-custom-apply"]');
    await waitFn(page(), () => document.querySelector('[data-testid="watch-button"]')?.textContent.includes("Custom"));
    let st = await pageFetchJson(page(), `/api/forge/repos/${full}/subscription`);
    assert(st.state === "custom" && st.events.join() === "issues", `custom watch: ${JSON.stringify(st)}`);
    await click('[data-testid="watch-button"]');
    await click('[data-testid="watch-option-watching"]');
    await waitFn(page(), () => document.querySelector('[data-testid="watch-button"]')?.textContent.includes("Unwatch"));
    st = await pageFetchJson(page(), `/api/forge/repos/${full}/subscription`);
    assert(st.state === "watching", `watching: ${JSON.stringify(st)}`);
    await click('[data-testid="watch-count"]');
    await page().waitForFunction((p) => location.pathname === p, {}, `/${full}/watchers`);
    await waitText("Watchers");
    await page().waitForSelector(`.user-grid a[href="/${me}"]`);
    await shot("social-02-watchers");
  });

  await step("social: follow from profile (followers / following tabs)", async () => {
    await goto(`/${me}`, p2);
    await p2.waitForSelector('[data-testid="follow-button"]');
    if ((await testText(p2, "follow-button")) === "Unfollow") {
      await click('[data-testid="follow-button"]', p2);
      await waitFn(p2, () => document.querySelector('[data-testid="follow-button"]')?.textContent === "Follow");
    }
    await click('[data-testid="follow-button"]', p2);
    await waitFn(p2, () => document.querySelector('[data-testid="follow-button"]')?.textContent === "Unfollow");
    await waitFn(p2, () => Number(document.querySelector('[data-testid="followers-count"]')?.textContent) >= 1);
    await shot("social-03-profile-follow", p2);
    await click('[data-testid="followers-link"]', p2);
    await p2.waitForSelector(`[data-testid="followers-list"] a[href="/${other}"]`);
    await goto(`/${other}?tab=following`, p2);
    await p2.waitForSelector(`[data-testid="following-list"] a[href="/${me}"]`);
    await shot("social-04-following", p2);
  });

  await step("social: star → stargazers page", async () => {
    await goto(`/${full}`, p2);
    await p2.waitForSelector('[data-testid="star-button"]');
    if (!(await testText(p2, "star-button")).includes("Starred")) await click('[data-testid="star-button"]', p2);
    await waitFn(p2, () => document.querySelector('[data-testid="star-button"]')?.textContent.includes("Starred"));
    await click('[data-testid="star-count"]', p2);
    await p2.waitForFunction((p) => location.pathname === p, {}, `/${full}/stargazers`);
    await p2.waitForSelector(`.user-grid a[href="/${other}"]`);
    await shot("social-05-stargazers", p2);
  });

  await step("social: other user opens an issue → bell dot + inbox → mark read / done", async () => {
    const r = await api(`/repos/${full}/issues`, { method: "POST", body: { title: `Social issue ${RUN}`, body: `Hello @${me}` } }, p2);
    assert(r.status === 201, `create issue ${r.status} ${r.text}`);
    issueNo = r.json.number;
    await until(async () => (await api("/notifications", {}, page())).json.some((n) => n.subject.title === `Social issue ${RUN}`), "notification created");
    await goto("/");
    await page().waitForSelector('[data-testid="notification-dot"]');
    await shot("social-06-bell");
    await click('[data-testid="notification-bell"]');
    await page().waitForFunction(() => location.pathname === "/notifications");
    await page().waitForFunction((title) => [...document.querySelectorAll('[data-testid="notification-row"]')].some((r) => r.textContent.includes(title)), {}, `Social issue ${RUN}`);
    const reason = await page().$$eval('[data-testid="notification-row"]', (rows, title) =>
      rows.find((r) => r.textContent.includes(title))?.querySelector('[data-testid="notification-reason"]')?.textContent, `Social issue ${RUN}`);
    assert(reason === "Mention" || reason === "Watching", `reason ${reason}`);
    await shot("social-07-inbox");
    // repo filter + reason filter
    await page().locator(`[data-testid="notif-repo-filter"]::-p-text(${full})`).click();
    await waitBody(new RegExp(`repo:${full}`));
    await waitText(`Social issue ${RUN}`);
    await goto("/notifications");
    await rowAction(page(), `Social issue ${RUN}`, "notification-mark-read");
    await waitFn(page(), (title) => ![...document.querySelectorAll('[data-testid="notification-row"]')].some((r) => r.textContent.includes(title)), `Social issue ${RUN}`);
    await click('[data-testid="notif-filter-all"]');
    await page().waitForFunction((title) => [...document.querySelectorAll('[data-testid="notification-row"]')].some((r) => r.textContent.includes(title) && r.dataset.unread === "0"), {}, `Social issue ${RUN}`);
    await click('[data-testid="notif-filter-participating"]');
    await waitText(`Social issue ${RUN}`); // mentioned → participating
    await goto("/notifications?filter=all");
    await rowAction(page(), `Social issue ${RUN}`, "notification-done");
    await waitFn(page(), (title) => ![...document.querySelectorAll('[data-testid="notification-row"]')].some((r) => r.textContent.includes(title)), `Social issue ${RUN}`);
    // A new comment brings the thread back (unread); "Mark all as read" clears it.
    await api(`/repos/${full}/issues/${issueNo}/comments`, { method: "POST", body: { body: "first follow-up" } }, p2);
    await until(async () => (await api("/notifications", {}, page())).json.some((n) => n.subject.title === `Social issue ${RUN}`), "thread unread again");
    await goto("/notifications");
    await waitText(`Social issue ${RUN}`);
    await click('[data-testid="notif-mark-all"]');
    await waitText("All caught up!");
    await shot("social-08-inbox-empty");
  });

  await step("social: unsubscribe from a thread in the inbox", async () => {
    await api(`/repos/${full}/issues/${issueNo}/comments`, { method: "POST", body: { body: "second follow-up" } }, p2);
    await until(async () => (await api("/notifications", {}, page())).json.some((n) => n.subject.title === `Social issue ${RUN}`), "thread unread");
    await goto("/notifications");
    await rowAction(page(), `Social issue ${RUN}`, "notification-unsubscribe");
    await waitFn(page(), (title) => ![...document.querySelectorAll('[data-testid="notification-row"]')].some((r) => r.textContent.includes(title)), `Social issue ${RUN}`);
    await api(`/repos/${full}/issues/${issueNo}/comments`, { method: "POST", body: { body: "third follow-up (muted)" } }, p2);
    await new Promise((r) => setTimeout(r, 1500));
    const n = await api("/notifications", {}, page());
    assert(!n.json.some((x) => x.subject.title === `Social issue ${RUN}`), "muted thread stays quiet");
  });

  await step("social: issue sidebar Subscribe / Unsubscribe", async () => {
    await goto(`/${full}/issues/${issueNo}`);
    await page().waitForSelector('[data-testid="subscribe-button"]');
    // Muted from the inbox → not subscribed.
    await waitFn(page(), () => document.querySelector('[data-testid="subscribe-button"]')?.textContent.includes("Subscribe"));
    await click('[data-testid="subscribe-button"]');
    await waitFn(page(), () => document.querySelector('[data-testid="subscribe-button"]')?.textContent.includes("Unsubscribe"));
    await waitBody(/subscribed to this thread/);
    await shot("social-09-issue-subscribe");
    await api(`/repos/${full}/issues/${issueNo}/comments`, { method: "POST", body: { body: "after resubscribe" } }, p2);
    await until(async () => (await api("/notifications", {}, page())).json.some((n) => n.subject.title === `Social issue ${RUN}`), "subscribed again → notified");
    await click('[data-testid="subscribe-button"]');
    await waitFn(page(), () => document.querySelector('[data-testid="subscribe-button"]')?.textContent.trim().endsWith("Subscribe") && !document.querySelector('[data-testid="subscribe-button"]')?.textContent.includes("Unsubscribe"));
    // the other user (author) sees "Unsubscribe" with the author reason
    await goto(`/${full}/issues/${issueNo}`, p2);
    await p2.waitForSelector('[data-testid="subscribe-button"]');
    await waitBody(/because you authored this issue/, p2);
  });

  await step("social: pull request sidebar subscription", async () => {
    const main = await api(`/repos/${full}/git/ref/heads/main`, {}, page());
    await api(`/repos/${full}/git/refs`, { method: "POST", body: { ref: "refs/heads/social-pr", sha: main.json.object.sha } }, page());
    await api(`/repos/${full}/contents/pr.txt`, { method: "PUT", body: { message: "pr file", content: Buffer.from("pr\n").toString("base64"), branch: "social-pr" } }, page());
    const pr = await api(`/repos/${full}/pulls`, { method: "POST", body: { title: `Social PR ${RUN}`, head: "social-pr", base: "main" } }, page());
    assert(pr.status === 201, `pr ${pr.status} ${pr.text}`);
    pullNo = pr.json.number;
    await goto(`/${full}/pull/${pullNo}`);
    await page().waitForSelector('[data-testid="subscribe-button"]');
    await waitBody(/because you authored this pull request/);
    await click('[data-testid="subscribe-button"]');
    await waitFn(page(), () => !document.querySelector('[data-testid="subscribe-button"]')?.textContent.includes("Unsubscribe"));
    await click('[data-testid="subscribe-button"]');
    await waitFn(page(), () => document.querySelector('[data-testid="subscribe-button"]')?.textContent.includes("Unsubscribe"));
    await shot("social-10-pr-subscribe");
  });

  await step("social: activity feed on the dashboard and profile", async () => {
    await goto("/");
    await waitText("Welcome");
    await page().waitForSelector('[data-testid="feed-following"] [data-testid="feed-item"]');
    await waitBody(new RegExp(`opened issue Social issue ${RUN}`));
    await shot("social-11-dashboard-feed");
    await click('[data-testid="feed-tab-mine"]');
    await page().waitForSelector('[data-testid="feed-mine"] [data-testid="feed-item"]');
    await waitBody(new RegExp(`created a repository ${full}`));
    // The follower sees the followed user's activity.
    await goto("/", p2);
    await p2.waitForSelector('[data-testid="feed-following"] [data-testid="feed-item"]');
    await waitBody(new RegExp(full), p2);
    await goto(`/${other}?tab=activity`);
    await page().waitForSelector('[data-testid="profile-activity"] [data-testid="feed-item"]');
    await waitBody(new RegExp(`opened issue Social issue ${RUN}`));
    await shot("social-12-profile-activity");
  });

  await step("social: gists — create (multiple files) via the user menu", async () => {
    await goto("/");
    await click('button[aria-label="Open user menu"]');
    await clickText("Your gists", "a");
    await page().waitForFunction((p) => location.pathname === p, {}, `/gist/${me}`);
    await click('[data-testid="new-gist-button"]');
    await page().waitForFunction(() => location.pathname === "/gists/new");
    await fill("#gist-desc", `Social gist ${RUN}`);
    await fill('[data-testid="gist-filename"]', "hello.py");
    await fill('[data-testid="gist-content"]', 'def hello(name):\n    return "hi " + name\n');
    await click('[data-testid="gist-add-file"]');
    const names = await page().$$('[data-testid="gist-filename"]');
    await names[1].type("notes.md");
    const contents = await page().$$('[data-testid="gist-content"]');
    await contents[1].type("# Notes\n\nSome *markdown*.");
    await shot("social-13-new-gist");
    await click('[data-testid="create-secret-gist"]');
    await page().waitForFunction(() => /^\/gist\/[^/]+\/[0-9a-f]+$/.test(location.pathname));
    gistId = (await page().evaluate(() => location.pathname)).split("/").pop();
    await page().waitForSelector('[data-testid="gist-secret"]');
    await waitText(`Social gist ${RUN}`);
    const n = await page().$$eval('[data-testid="gist-file"]', (e) => e.length);
    assert(n === 2, `gist files ${n}`);
    await page().waitForSelector('[data-testid="gist-file"] .hljs-keyword'); // syntax highlighting
    await page().waitForSelector('[data-testid="gist-file"] .markdown-body h1'); // markdown rendered
    const rawHref = await page().$eval('[data-testid="gist-raw-link"]', (a) => a.getAttribute("href"));
    const raw = await t.pageFetch(rawHref);
    assert(raw.status === 200 && raw.text.includes("def hello"), `raw ${raw.status} ${raw.text.slice(0, 80)}`);
    await page().select('[data-testid="gist-share-kind"]', "embed");
    const embed = await page().$eval('[data-testid="gist-share-url"]', (e) => e.value);
    assert(embed.startsWith("<iframe"), `embed ${embed}`);
    await shot("social-14-gist");
  });

  await step("social: gists — edit (rename, change, add) → revisions with diff", async () => {
    await click('[data-testid="edit-gist"]');
    await page().waitForFunction(() => location.pathname.endsWith("/edit"));
    await page().waitForSelector('[data-testid="gist-content"]');
    await setValue(page(), '[data-testid="gist-filename"]', "greet.py");
    const contents = await page().$$('[data-testid="gist-content"]');
    await contents[0].click();
    await page().keyboard.down("End");
    await contents[0].type("\nprint(hello('e2e'))\n");
    await click('[data-testid="update-gist"]');
    await page().waitForFunction((id) => location.pathname.endsWith(`/${id}`), {}, gistId);
    await waitFn(page(), () => [...document.querySelectorAll('[data-testid="gist-file-name"]')].map((e) => e.textContent).sort().join() === "greet.py,notes.md");
    await click('[data-testid="gist-tab-revisions"]');
    await page().waitForFunction(() => location.pathname.endsWith("/revisions"));
    await waitFn(page(), () => document.querySelectorAll('[data-testid="gist-revision"]').length === 2);
    await page().waitForSelector(".gist-diff .diff-add");
    await shot("social-15-gist-revisions");
    const views = await page().$$('[data-testid="gist-revision-view"]');
    await views[1].click(); // the first (oldest) revision
    await waitBody(/Viewing revision/);
    await waitText("hello.py");
  });

  await step("social: gists — star, fork, comments by another user", async () => {
    await goto(`/gist/${me}/${gistId}`, p2);
    await p2.waitForSelector('[data-testid="star-gist"]');
    await click('[data-testid="star-gist"]', p2);
    await waitFn(p2, () => document.querySelector('[data-testid="star-gist"]')?.textContent.includes("Unstar"));
    await fill("#gist-new-comment", "Nice **gist**!", p2);
    await click('[data-testid="md-preview-tab"]', p2);
    await p2.waitForSelector('[data-testid="md-preview"] strong');
    await click('[data-testid="gist-comment-submit"]', p2);
    await p2.waitForSelector('[data-testid="gist-comment"] strong::-p-text(gist)');
    await click('[data-testid="gist-comment-edit"]', p2);
    await p2.waitForSelector('[data-testid="gist-comment"] textarea');
    await p2.locator('[data-testid="gist-comment"] textarea').fill("Edited comment");
    await click('[data-testid="gist-comment-save"]', p2);
    await waitBody(/Edited comment/, p2);
    await shot("social-16-gist-comments", p2);
    await click('[data-testid="gist-comment-delete"]', p2);
    await waitFn(p2, () => document.querySelectorAll('[data-testid="gist-comment"]').length === 0);
    await goto("/gists/starred", p2);
    await p2.waitForSelector(`a[href="/gist/${me}/${gistId}"]`);
    await goto(`/gist/${me}/${gistId}`, p2);
    await click('[data-testid="fork-gist"]', p2);
    await p2.waitForFunction((id) => /^\/gist\/[^/]+\/[0-9a-f]+$/.test(location.pathname) && !location.pathname.endsWith(id), {}, gistId);
    await waitBody(/forked from/, p2);
    await goto(`/gist/${me}/${gistId}/forks`);
    await page().waitForSelector(`[data-testid="gist-forks"] a[href^="/gist/${other}/"]`);
    await shot("social-17-gist-forks");
  });

  await step("social: gists — public gist on discover, then delete", async () => {
    await goto("/gists/new");
    await fill("#gist-desc", `Public social gist ${RUN}`);
    await fill('[data-testid="gist-filename"]', "public.txt");
    await fill('[data-testid="gist-content"]', "public content\n");
    await click('[data-testid="create-public-gist"]');
    await page().waitForFunction(() => /^\/gist\/[^/]+\/[0-9a-f]+$/.test(location.pathname));
    publicGistId = (await page().evaluate(() => location.pathname)).split("/").pop();
    await goto("/gists/discover", p2);
    await p2.waitForSelector(`a[href="/gist/${me}/${publicGistId}"]`);
    await shot("social-18-gist-discover", p2);
    await goto(`/gist/${me}`, p2); // other users only see public gists
    await p2.waitForSelector(`a[href="/gist/${me}/${publicGistId}"]`);
    assert(!(await p2.$(`a[href="/gist/${me}/${gistId}"]`)), "secret gist not listed for others");
    for (const id of [publicGistId, gistId]) {
      await goto(`/gist/${me}/${id}`);
      await click('[data-testid="delete-gist"]');
      await page().waitForFunction((p) => location.pathname === p, {}, `/gist/${me}`);
    }
    const gone = await api(`/gists/${gistId}`, {}, page());
    assert(gone.status === 404, `deleted gist ${gone.status}`);
  });

  await step("social: settings — public profile shows on the profile page", async () => {
    await goto("/settings");
    await page().waitForSelector('[data-testid="profile-form"]');
    await fill("#pf-name", `Social ${RUN}`);
    await fill("#pf-bio", `Bio ${RUN}`);
    await fill("#pf-company", "Forge Inc");
    await fill("#pf-location", "Seattle");
    await fill("#pf-blog", "https://example.com/blog");
    await fill("#pf-twitter_username", `tw${RUN}`);
    const hire = await page().$eval('[data-testid="pf-hireable"]', (e) => e.checked);
    if (!hire) await click('[data-testid="pf-hireable"]');
    await click('[data-testid="profile-save"]');
    await waitText("Profile updated successfully.");
    await fill("#social-url", `https://mastodon.social/@e2e${RUN}`);
    await click('[data-testid="social-add"]');
    await page().waitForSelector('[data-testid="social-row"]');
    await shot("social-19-settings-profile");
    await goto(`/${me}`);
    await waitText(`Social ${RUN}`);
    await waitBody(new RegExp(`Bio ${RUN}`));
    await waitBody(/Forge Inc/);
    await page().waitForSelector('[data-testid="hireable"]');
    await page().waitForSelector('[data-testid="social-account"]');
    await waitBody(new RegExp(`@tw${RUN}`));
    await goto("/settings/profile");
    await page().waitForSelector('[data-testid="social-row"]');
    while (await page().$('[data-testid="social-remove"]')) {
      const n = await page().$$eval('[data-testid="social-row"]', (e) => e.length);
      await click('[data-testid="social-remove"]');
      await waitFn(page(), (k) => document.querySelectorAll('[data-testid="social-row"]').length < k, n);
    }
  });

  await step("social: settings — emails (add, make primary, remove)", async () => {
    const extra = `${me.toLowerCase()}+${RUN}@example.com`;
    await goto("/settings/emails");
    await page().waitForSelector('[data-testid="email-row"]');
    const original = await page().$eval('[data-testid="email-row"]:has([data-testid="email-primary"]) [data-testid="email-address"]', (e) => e.textContent);
    await fill('[data-testid="email-input"]', extra);
    await click('[data-testid="email-add"]');
    await waitText(extra);
    await clickInRow(page(), "email-row", extra, "email-make-primary");
    await waitFn(page(), (a) => [...document.querySelectorAll('[data-testid="email-row"]')].some((r) => r.textContent.includes(a) && r.querySelector('[data-testid="email-primary"]')), extra);
    await shot("social-20-emails");
    await clickInRow(page(), "email-row", original, "email-make-primary");
    await waitFn(page(), (a) => [...document.querySelectorAll('[data-testid="email-row"]')].some((r) => r.textContent.includes(a) && r.querySelector('[data-testid="email-primary"]')), original);
    await clickInRow(page(), "email-row", extra, "email-remove");
    await waitFn(page(), (a) => ![...document.querySelectorAll('[data-testid="email-row"]')].some((r) => r.textContent.includes(a)), extra);
  });

  await step("social: settings — change password (and back)", async () => {
    await goto("/settings/security");
    await fill('[data-testid="pw-old"]', "wrong-password-1");
    await fill('[data-testid="pw-new"]', `${PASSWORD}-x`);
    await fill('[data-testid="pw-confirm"]', `${PASSWORD}-x`);
    await click('[data-testid="pw-submit"]');
    await waitBody(/current_password invalid|invalid/i);
    await fill('[data-testid="pw-old"]', PASSWORD);
    await click('[data-testid="pw-submit"]');
    await waitText("Password changed successfully.");
    const ok = await t.pageFetch("/api/forge/session", { method: "POST", headers: { "Content-Type": "application/json", "X-Forge-CSRF": "1" }, body: JSON.stringify({ login: me, password: `${PASSWORD}-x` }) });
    assert(ok.status === 200, `login with new password ${ok.status}`);
    await fill('[data-testid="pw-old"]', `${PASSWORD}-x`);
    await fill('[data-testid="pw-new"]', PASSWORD);
    await fill('[data-testid="pw-confirm"]', PASSWORD);
    await click('[data-testid="pw-submit"]');
    await waitText("Password changed successfully.");
    await shot("social-21-password");
  });

  await step("social: settings — GPG key and SSH signing key add/delete", async () => {
    const key = gpgPublicKey(`E2E ${RUN} <${me.toLowerCase()}@example.com>`);
    await goto("/settings/gpg_keys");
    await fill('[data-testid="gpg-name"]', `e2e ${RUN}`);
    await setValue(page(), '[data-testid="gpg-key-input"]', key.armored);
    await click('[data-testid="gpg-add"]');
    await page().waitForSelector(`[data-testid="gpg-key-id"]::-p-text(${key.keyId})`);
    const keyPath = join(WORK, `signing-${RUN}`);
    const kg = spawnSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-C", `e2e-${RUN}`, "-f", keyPath]);
    assert(kg.status === 0, `ssh-keygen: ${kg.stderr}`);
    await fill('[data-testid="signing-title"]', `signing ${RUN}`);
    await setValue(page(), '[data-testid="signing-key-input"]', readFileSync(`${keyPath}.pub`, "utf8").trim());
    await click('[data-testid="signing-add"]');
    await waitText(`signing ${RUN}`);
    await shot("social-22-signing-keys");
    await clickInRow(page(), "gpg-key-row", key.keyId, "gpg-key-delete");
    await waitFn(page(), (k) => !document.body.innerText.includes(k), key.keyId);
    await clickInRow(page(), "signing-key-row", `signing ${RUN}`, "signing-key-delete");
    await waitFn(page(), (k) => !document.body.innerText.includes(k), `signing ${RUN}`);
  });

  await step("social: settings — appearance matches the header toggle", async () => {
    await goto("/settings/appearance");
    await click('[data-testid="theme-dark"]');
    await waitFn(page(), () => document.documentElement.dataset.theme === "dark");
    const label = await page().$eval('[data-testid="theme-toggle"]', (e) => e.getAttribute("aria-label"));
    assert(label.startsWith("Theme: dark"), `toggle label ${label}`);
    await shot("social-23-appearance-dark");
    await click('[data-testid="theme-toggle"]'); // dark → auto
    await waitFn(page(), () => document.querySelector('[data-testid="theme-auto"] input')?.checked);
    await click('[data-testid="theme-light"]');
    await waitFn(page(), () => document.documentElement.dataset.theme === "light");
    await click('[data-testid="theme-auto"]');
    await waitFn(page(), () => !document.documentElement.dataset.theme);
  });

  await step("social: block user (profile + settings), blocked user can't follow or comment", async () => {
    await goto(`/${other}`);
    await page().waitForSelector('[data-testid="block-button"]');
    if ((await testText(page(), "block-button")).includes("Unblock")) {
      await click('[data-testid="block-button"]');
      await waitFn(page(), () => document.querySelector('[data-testid="block-button"]')?.textContent.includes("Block user"));
    }
    await click('[data-testid="block-button"]');
    await page().waitForSelector('[data-testid="blocked-note"]');
    const f = await api(`/user/following/${me}`, { method: "PUT" }, p2);
    assert(f.status === 403, `blocked follow ${f.status}`);
    const c = await api(`/repos/${full}/issues/${issueNo}/comments`, { method: "POST", body: { body: "blocked" } }, p2);
    assert(c.status === 403, `blocked comment ${c.status}`);
    const fl = await api(`/users/${me}/followers`, {}, p2);
    assert(!fl.json.some((u) => u.login === other), "block removed the follow");
    await goto("/settings/blocked_users");
    await page().waitForSelector(`[data-testid="blocked-row"] a[href="/${other}"]`);
    await shot("social-24-blocked");
    await click('[data-testid="unblock-button"]');
    await waitText("You have not blocked any users.");
    await fill('[data-testid="block-input"]', other);
    await click('[data-testid="block-submit"]');
    await page().waitForSelector(`[data-testid="blocked-row"] a[href="/${other}"]`);
    await click('[data-testid="unblock-button"]');
    await waitText("You have not blocked any users.");
    const f2 = await api(`/user/following/${me}`, { method: "PUT" }, p2);
    assert(f2.status === 204, `follow after unblock ${f2.status}`);
  });

  await step("social: account — export data", async () => {
    await goto("/settings/admin");
    await click('[data-testid="export-start"]');
    await page().waitForSelector('[data-testid="export-list"]');
    await page().waitForSelector('[data-testid="export-download"]', { timeout: 60000 });
    const href = await page().$eval('[data-testid="export-download"]', (a) => a.getAttribute("href"));
    const res = await page().evaluate(async (u) => { const r = await fetch(u, { credentials: "same-origin" }); return { status: r.status, ct: r.headers.get("content-type"), n: (await r.arrayBuffer()).byteLength }; }, href);
    assert(res.status === 200 && res.n > 0, `archive ${JSON.stringify(res)}`);
    await shot("social-25-account");
  });

  await step("social: account — rename (and back) for the second user", async () => {
    const renamed = `${other}-r${RUN}`.slice(0, 39);
    await goto("/settings/admin", p2);
    await fill('[data-testid="rename-input"]', renamed, p2);
    await click('[data-testid="rename-submit"]', p2);
    await waitText("Your username has been changed.", p2);
    const u = await api("/user", {}, p2);
    assert(u.json.login === renamed, `renamed ${u.json.login}`);
    await goto(`/${renamed}`, p2);
    await waitBody(new RegExp(renamed), p2);
    await goto("/settings/admin", p2);
    await fill('[data-testid="rename-input"]', other, p2);
    await click('[data-testid="rename-submit"]', p2);
    await waitText("Your username has been changed.", p2);
  });

  await step("social: account — delete a throwaway account", async () => {
    const ctx = await t.browser.createBrowserContext();
    const p3 = await ctx.newPage();
    p3.setDefaultTimeout(45000);
    p3.on("dialog", (d) => d.accept());
    const login = `del${RUN}${Math.floor(Math.random() * 1e4)}`;
    await t.signUpOrIn(p3, login, PASSWORD);
    await goto("/settings/admin", p3);
    await fill('[data-testid="delete-login"]', login, p3);
    await fill('[data-testid="delete-password"]', PASSWORD, p3);
    await click('[data-testid="delete-account"]', p3);
    try {
      await waitText("Where code gets forged", p3);
    } catch (e) {
      await shot("FAIL-social-delete-account-p3", p3);
      throw e;
    }
    const r = await api(`/users/${login}`, {}, page());
    assert(r.status === 404, `deleted user ${r.status}`);
    await ctx.close();
  });

  await step("social: /watching lists and unwatches", async () => {
    await goto("/watching");
    await page().waitForSelector(`[data-testid="watching-list"] a[href="/${full}"]`);
    await shot("social-26-watching");
    await clickInRow(page(), "watching-list li", full, "unwatch-button", ".watching-row");
    await waitFn(page(), (f) => !document.querySelector(`[data-testid="watching-list"] a[href="/${f}"]`), full);
    await goto(`/${full}`);
    await waitFn(page(), () => document.querySelector('[data-testid="watch-button"]')?.textContent.includes("Watch") && !document.querySelector('[data-testid="watch-button"]')?.textContent.includes("Unwatch"));
  });

  await step("social: explore (repositories, people)", async () => {
    await goto("/");
    await page().locator('.header-nav a::-p-text(Explore)').click();
    await page().waitForFunction(() => location.pathname === "/explore");
    await page().waitForSelector('[data-testid="explore-repos"]');
    await click('[data-testid="explore-tab-updated"]');
    await page().waitForSelector(`[data-testid="explore-repos"] a[href="/${full}"]`);
    await click('[data-testid="explore-tab-people"]');
    await fill('[data-testid="explore-people-q"]', other);
    await page().keyboard.press("Enter");
    await page().waitForSelector(`[data-testid="explore-people"] a[href="/${other}"]`);
    await shot("social-27-explore");
  });

  await step("social: pages at 375px have no horizontal overflow", async () => {
    const m = await t.page.browserContext().newPage();
    m.setDefaultNavigationTimeout(60000);
    await m.setViewport({ width: 375, height: 800, isMobile: true, hasTouch: true });
    const pages = [
      ["social-m-dashboard", "/"],
      ["social-m-notifications", "/notifications?filter=all"],
      ["social-m-profile", `/${other}`],
      ["social-m-followers", `/${me}?tab=followers`],
      ["social-m-stargazers", `/${full}/stargazers`],
      ["social-m-gists", "/gists/discover"],
      ["social-m-gist-new", "/gists/new"],
      ["social-m-settings-profile", "/settings/profile"],
      ["social-m-settings-emails", "/settings/emails"],
      ["social-m-settings-gpg", "/settings/gpg_keys"],
      ["social-m-explore", "/explore"],
      ["social-m-watching", "/watching"],
    ];
    for (const [name, p] of pages) {
      await goto(p, m);
      const over = await m.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      await shot(name, m);
      assert(over <= 1, `${p} overflows by ${over}px`);
    }
    // a gist page with a long line of code scrolls inside the code box only
    const g = await api("/gists", { method: "POST", body: { description: "mobile", public: false, files: { "wide.js": { content: `const x = "${"w".repeat(300)}";\n` } } } }, page());
    await goto(`/gist/${me}/${g.json.id}`, m);
    await m.waitForSelector('[data-testid="gist-file"]');
    const over = await m.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    await shot("social-m-gist", m);
    await api(`/gists/${g.json.id}`, { method: "DELETE" }, page());
    assert(over <= 1, `gist page overflows by ${over}px`);
    await m.close();
  });

  // ---------- helpers ----------
  /** Set a (long) input value at once, firing React's onChange. */
  async function setValue(p, sel, value) {
    await p.waitForSelector(sel);
    await p.$eval(sel, (el, v) => {
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value").set.call(el, v);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }, value);
  }
  async function pageFetchJson(p, path) {
    const r = await t.pageFetch(path, {}, p);
    return JSON.parse(r.text);
  }
  /** Click the button `btn` (data-testid) inside the row containing `text`. */
  async function rowAction(p, text, btn) {
    await clickInRow(p, "notification-row", text, btn);
  }
  async function clickInRow(p, rowId, text, btn, rowSel) {
    const sel = rowSel || `[data-testid="${rowId}"]`;
    await p.waitForFunction((s, x) => [...document.querySelectorAll(s)].some((r) => r.textContent.includes(x)), {}, sel, text);
    const ok = await p.evaluate((s, x, b) => {
      const row = [...document.querySelectorAll(s)].find((r) => r.textContent.includes(x));
      const el = row?.querySelector(`[data-testid="${b}"]`);
      if (!el) return false;
      el.click();
      return true;
    }, sel, text, btn);
    assert(ok, `no ${btn} in row with ${text}`);
  }
}

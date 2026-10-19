// E2E flows of the packages area: packages (npm + container) published to the backend, browsed,
// deleted/restored and configured in the UI; GitHub Apps (register, keys, permissions, install,
// installation tokens minted with a JWT signed here, suspend, uninstall, delete); OAuth apps
// (CRUD, device flow, authorized apps, revoke); org packages; org settings sections when the
// org settings shell (orgs area) is present; 375 px overflow checks.
//
// Registries (/v2/, /npm/) and the device-flow endpoints are not proxied by Vite: they are
// called on the backend directly (--backend URL, else FORGE_BACKEND, else the area port scheme
// vite 52x3 -> backend 34x0).
import { createHash, createSign } from "node:crypto";
import { existsSync, readdirSync } from "node:fs";
import { gzipSync } from "node:zlib";

const b64url = (b) => Buffer.from(b).toString("base64").replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");

/** A minimal ustar archive of `{path: content}`. */
function tar(files) {
  const blocks = [];
  for (const [name, content] of Object.entries(files)) {
    const data = Buffer.from(content);
    const h = Buffer.alloc(512);
    h.write(name, 0, 100);
    h.write("0000644\0", 100);
    h.write("0000000\0", 108);
    h.write("0000000\0", 116);
    h.write(data.length.toString(8).padStart(11, "0") + "\0", 124);
    h.write(Math.floor(Date.now() / 1000).toString(8).padStart(11, "0") + "\0", 136);
    h.write("        ", 148);
    h.write("0", 156);
    h.write("ustar\0", 257);
    h.write("00", 263);
    let sum = 0;
    for (const b of h) sum += b;
    h.write(sum.toString(8).padStart(6, "0") + "\0 ", 148);
    blocks.push(h, data, Buffer.alloc((512 - (data.length % 512)) % 512));
  }
  blocks.push(Buffer.alloc(1024));
  return Buffer.concat(blocks);
}

function jwt(appId, pem) {
  const now = Math.floor(Date.now() / 1000);
  const head = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
  const body = b64url(JSON.stringify({ iat: now - 60, exp: now + 540, iss: String(appId) }));
  const sig = createSign("RSA-SHA256").update(`${head}.${body}`).sign(pem);
  return `${head}.${body}.${b64url(sig)}`;
}

export async function run(t) {
  const { step, goto, waitText, waitBody, click, fill, clickText, shot, assert, log, api, BASE, RUN, args } = t;
  const page = () => t.page;
  const me = t.me;
  const lme = me.toLowerCase();
  const port = Number(new URL(BASE).port || 80);
  let BACKEND = String(args.backend || process.env.FORGE_BACKEND || "").replace(/\/$/, "");
  const NPM_NAME = `e2e-lib-${RUN}`;
  const IMG = `img-${RUN}`;
  const REPO = `pkgrepo-${RUN}`;
  const OTHER = `pkgother-${RUN}`;
  const ORG = `pkorg-${RUN}`;
  const APP = `E2E App ${RUN}`;
  const OAPP = `E2E OAuth ${RUN}`;
  let appSlug = "";
  let appId = 0;
  let pem = "";
  let instId = 0;
  let clientId = "";

  const raw = async (method, url, { body, headers = {}, auth = true } = {}) => {
    const h = { ...headers };
    if (auth) h.Authorization = `Bearer ${t.token}`;
    const r = await fetch(url, { method, headers: h, body });
    const text = await r.text();
    let json = null;
    try { json = text ? JSON.parse(text) : null; } catch { /* not JSON */ }
    return { status: r.status, headers: r.headers, text, json };
  };
  const digest = (b) => `sha256:${createHash("sha256").update(b).digest("hex")}`;

  async function pushBlob(name, data) {
    const d = digest(data);
    const r = await raw("POST", `${BACKEND}/v2/${name}/blobs/uploads/?digest=${d}`, { body: data, headers: { "Content-Type": "application/octet-stream" } });
    assert(r.status === 201, `blob upload ${name}: ${r.status} ${r.text}`);
    return d;
  }
  async function pushImage(name, tag, labels) {
    const layer = Buffer.from(`layer ${tag} ${RUN} ${Math.random()}`);
    const config = Buffer.from(JSON.stringify({ architecture: "amd64", os: "linux", config: { Labels: labels }, rootfs: { type: "layers", diff_ids: [digest(layer)] } }));
    const ld = await pushBlob(name, layer);
    const cd = await pushBlob(name, config);
    const manifest = Buffer.from(JSON.stringify({
      schemaVersion: 2,
      mediaType: "application/vnd.oci.image.manifest.v1+json",
      config: { mediaType: "application/vnd.oci.image.config.v1+json", digest: cd, size: config.length },
      layers: [{ mediaType: "application/vnd.oci.image.layer.v1.tar+gzip", digest: ld, size: layer.length }],
    }));
    const r = await raw("PUT", `${BACKEND}/v2/${name}/manifests/${tag}`, { body: manifest, headers: { "Content-Type": "application/vnd.oci.image.manifest.v1+json" } });
    assert(r.status === 201, `manifest push ${name}:${tag}: ${r.status} ${r.text}`);
    return digest(manifest);
  }
  async function npmPublish(scope, name, version, readme, repository) {
    const full = `@${scope}/${name}`;
    const pkgJson = { name: full, version, description: `e2e package ${name}`, license: "MIT", main: "index.js", ...(repository ? { repository: { type: "git", url: repository } } : {}) };
    const tgz = gzipSync(tar({ "package/package.json": JSON.stringify(pkgJson, null, 2), "package/README.md": readme, "package/index.js": "module.exports = 42;\n" }));
    const file = `${name}-${version}.tgz`;
    const doc = {
      _id: full, name: full, description: pkgJson.description, "dist-tags": { latest: version },
      versions: { [version]: { ...pkgJson, _id: `${full}@${version}`, dist: { shasum: createHash("sha1").update(tgz).digest("hex"), integrity: `sha512-${createHash("sha512").update(tgz).digest("base64")}`, tarball: `${BACKEND}/npm/${full}/-/${file}` } } },
      _attachments: { [file]: { content_type: "application/octet-stream", data: tgz.toString("base64"), length: tgz.length } },
    };
    const r = await raw("PUT", `${BACKEND}/npm/${full.replace("/", "%2f")}`, { body: JSON.stringify(doc), headers: { "Content-Type": "application/json" } });
    assert(r.status === 200 || r.status === 201, `npm publish ${full}@${version}: ${r.status} ${r.text}`);
  }
  /** REST status for a path, unauthenticated (from node). */
  const anon = async (path) => (await fetch(`${BASE}/api/v3${path}`, { headers: { Accept: "application/vnd.github+json" } })).status;
  const settingsShell = async () => !!(await page().$(".settings-nav"));

  await step("packages: locate the backend (registries are not proxied by Vite)", async () => {
    const candidates = [BACKEND, BASE, `http://localhost:${port - 1803}`].filter(Boolean);
    BACKEND = "";
    for (const c of candidates) {
      try {
        const r = await fetch(`${c}/v2/`);
        if (r.headers.get("docker-distribution-api-version")) { BACKEND = c.replace(/\/$/, ""); break; }
      } catch { /* not this one */ }
    }
    assert(BACKEND, `no registry at ${candidates.join(", ")} (pass --backend http://localhost:PORT)`);
    log(`    backend ${BACKEND}`);
  });

  await step("packages: publish npm versions and push container images", async () => {
    await t.newRepo(REPO, { private: true });
    await t.newRepo(OTHER, { private: true });
    await npmPublish(lme, NPM_NAME, "1.0.0", `# ${NPM_NAME}\n\nHello from the e2e README, version one.\n`);
    await npmPublish(lme, NPM_NAME, "1.1.0", `# ${NPM_NAME}\n\nHello from the e2e README.\n\n\`\`\`js\nrequire("@${lme}/${NPM_NAME}")\n\`\`\`\n`);
    const src = { "org.opencontainers.image.source": `${BASE}/${me}/${REPO}`, "org.opencontainers.image.description": "An e2e test image" };
    await pushImage(`${lme}/${IMG}`, "v1", src);
    await pushImage(`${lme}/${IMG}`, "latest", src);
    const p = await t.rest(`/users/${me}/packages/container/${IMG}`);
    assert(p.status === 200 && p.json.version_count === 2, `container package: ${p.status} ${p.text}`);
    assert(p.json.repository?.full_name === `${me}/${REPO}`, `image linked to repo: ${p.json.repository?.full_name}`);
  });

  await step("packages: profile Packages tab lists and filters packages", async () => {
    await goto(`/${me}`);
    await click('[data-testid="profile-tab-packages"]');
    await waitText(NPM_NAME);
    await waitText(IMG);
    await page().select('[data-testid="pkg-type-filter"]', "npm");
    await page().waitForFunction((img) => !document.body.innerText.includes(img), {}, IMG);
    await waitText(NPM_NAME);
    await fill('[data-testid="pkg-search"]', "no-such-package-zz");
    await page().keyboard.press("Enter");
    await waitText("No packages match your filters.");
    await goto(`/${me}?tab=packages`);
    await waitText(IMG);
    await shot("pkg-01-profile-packages");
  });

  await step("packages: npm package page shows README, install command, versions", async () => {
    await clickText(NPM_NAME, "a");
    await page().waitForSelector('[data-testid="pkg-readme"]');
    await waitText("Hello from the e2e README.");
    const cmd = await page().$eval('input[aria-label="npm install command"]', (e) => e.value);
    assert(cmd === `npm install @${lme}/${NPM_NAME}@1.1.0`, `install command ${cmd}`);
    const npmrc = await page().$eval('input[aria-label=".npmrc registry line"]', (e) => e.value);
    assert(npmrc.endsWith("/npm/") && npmrc.startsWith(`@${lme}:registry=`), npmrc);
    assert(new URL(page().url()).pathname === `/users/${me}/packages/npm/package/${NPM_NAME}`, page().url());
    await shot("pkg-02-npm-package");
    await click('[data-testid="pkg-tab-versions"]');
    await waitText("1.0.0");
    await waitText("1.1.0");
    await clickText("1.0.0", "a");
    await page().waitForSelector('[data-testid="version-digest"]');
    await waitText("version one");
    await shot("pkg-03-npm-version");
  });

  await step("packages: container page shows docker pull, tags and digests", async () => {
    await goto(`/users/${me}/packages/container/package/${IMG}`);
    const cmd = await page().$eval('input[aria-label="docker pull command"]', (e) => e.value);
    assert(cmd === `${new URL(BASE).host}/${lme}/${IMG}:latest`.replace(/^/, "docker pull "), `pull command ${cmd}`);
    await waitText("An e2e test image");
    await page().waitForSelector('[data-testid="pkg-linked-repo"]');
    await shot("pkg-04-container-package");
    await click('[data-testid="pkg-tab-versions"]');
    await waitText("v1");
    await clickText("v1", "a");
    await page().waitForSelector('[data-testid="version-digest"]');
    const d = await page().$eval('[data-testid="version-digest"]', (e) => e.textContent);
    assert(/^sha256:[0-9a-f]{64}$/.test(d), `digest ${d}`);
    await shot("pkg-05-container-version");
  });

  await step("packages: delete and restore a version", async () => {
    await goto(`/users/${me}/packages/npm/${NPM_NAME}/versions`);
    const rows = await page().$$('[data-testid="pkg-version-row"]');
    assert(rows.length === 2, `2 versions, got ${rows.length}`);
    // the oldest (1.0.0) is listed last
    const del = await page().$$('[data-testid="version-delete"]');
    await del[del.length - 1].click();
    await waitText("Version 1.0.0 deleted.");
    let v = await t.rest(`/users/${me}/packages/npm/${NPM_NAME}/versions`);
    assert(v.json.length === 1, `active versions after delete: ${v.json.length}`);
    await click('[data-testid="versions-deleted"]');
    await waitText("Deleted versions");
    await shot("pkg-06-deleted-versions");
    await click('[data-testid="version-restore"]');
    await waitText("Version 1.0.0 restored.");
    v = await t.rest(`/users/${me}/packages/npm/${NPM_NAME}/versions`);
    assert(v.json.length === 2, `active versions after restore: ${v.json.length}`);
  });

  await step("packages: settings link a repository, change visibility, inherit access", async () => {
    await goto(`/users/${me}/packages/npm/package/${NPM_NAME}`);
    await click('[data-testid="pkg-settings-link"]');
    await page().waitForSelector('[data-testid="pkg-settings"]');
    await fill("#pkg-repo", `${me}/${REPO}`);
    await click('[data-testid="pkg-link-save"]');
    await waitText(`Linked to ${me}/${REPO}.`);
    let p = await t.rest(`/users/${me}/packages/npm/${NPM_NAME}`);
    assert(p.json.repository?.full_name === `${me}/${REPO}`, `linked: ${p.text}`);
    assert(await anon(`/users/${me}/packages/npm/${NPM_NAME}`) === 404, "private package hidden from anonymous");
    await page().select('[data-testid="pkg-visibility-select"]', "public");
    await click('[data-testid="pkg-visibility-save"]');
    await waitText("Package visibility changed to public.");
    assert(await anon(`/users/${me}/packages/npm/${NPM_NAME}`) === 200, "public package readable anonymously");
    await click('[data-testid="pkg-inherit"]');
    await waitText("Access is no longer inherited.");
    p = await t.rest(`/users/${me}/packages/npm/${NPM_NAME}`);
    await click('[data-testid="pkg-inherit"]');
    await waitText("Access is inherited from the repository.");
    await shot("pkg-07-package-settings");
  });

  await step("packages: manage access grants a second user read on a private image", async () => {
    const p2 = await t.page2();
    const path = `/api/v3/users/${me}/packages/container/${IMG}`;
    const before = await t.pageFetch(path, {}, p2);
    assert(before.status === 404, `second user before grant: ${before.status}`);
    await goto(`/users/${me}/packages/container/${IMG}/settings`);
    await fill('[data-testid="pkg-access-login"]', t.LOGIN2);
    await click('[data-testid="pkg-access-add"]');
    await page().waitForSelector('[data-testid="pkg-access-row"]');
    const after = await t.pageFetch(path, {}, p2);
    assert(after.status === 200, `second user after grant: ${after.status}`);
    await shot("pkg-08-manage-access");
    await clickText("Remove", "button");
    await waitText("No people have been granted access");
    const gone = await t.pageFetch(path, {}, p2);
    assert(gone.status === 404, `second user after removal: ${gone.status}`);
  });

  await step("packages: repository sidebar and repository Packages page", async () => {
    await goto(`/${me}/${REPO}`);
    await page().waitForSelector('[data-testid="repo-packages-sidebar"] a[href*="/packages/"]');
    const side = await page().$eval('[data-testid="repo-packages-sidebar"]', (e) => e.innerText);
    assert(side.includes(IMG) && side.includes(NPM_NAME), `sidebar: ${side}`);
    await clickText("Packages", '[data-testid="repo-packages-sidebar"] a');
    await waitBody(new RegExp(`${me}/${IMG}`));
    assert(new URL(page().url()).pathname === `/${me}/${REPO}/packages`, page().url());
    await shot("pkg-09-repo-packages");
    await goto(`/${me}/${OTHER}`);
    await page().waitForSelector('[data-testid="repo-packages-sidebar"]');
    await waitText("No packages published");
  });

  await step("packages: delete a package and restore it from Settings → Packages", async () => {
    await goto(`/users/${me}/packages/npm/${NPM_NAME}/settings`);
    await fill("#pkg-del-confirm", NPM_NAME);
    await click('[data-testid="pkg-delete"]');
    await page().waitForFunction(() => location.search.includes("tab=packages"));
    assert((await t.rest(`/users/${me}/packages/npm/${NPM_NAME}`)).status === 404, "package deleted");
    await goto("/settings/packages");
    await page().waitForSelector('[data-testid="deleted-package-row"]');
    await shot("pkg-10-deleted-packages");
    const rows = await page().$$('[data-testid="deleted-package-row"]');
    let restored = false;
    for (const r of rows) {
      if ((await r.evaluate((e) => e.innerText)).includes(NPM_NAME)) {
        await (await r.$('[data-testid="package-restore"]')).click();
        restored = true;
        break;
      }
    }
    assert(restored, "deleted package listed");
    await waitText(`Package ${NPM_NAME} restored.`);
    const p = await t.rest(`/users/${me}/packages/npm/${NPM_NAME}`);
    assert(p.status === 200 && p.json.version_count === 2, `restored: ${p.status} ${p.text}`);
  });

  await step("packages: organization packages page", async () => {
    const r = await api("/../forge/orgs", { method: "POST", body: { login: ORG } });
    assert(r.status === 201 || r.status === 422, `create org: ${r.status} ${r.text}`);
    await pushImage(`${ORG.toLowerCase()}/org-img`, "1.0", {});
    await goto(`/orgs/${ORG}/packages`);
    await waitText("org-img");
    await shot("pkg-11-org-packages");
    await clickText("org-img", "a");
    await page().waitForFunction(() => location.pathname.includes("/orgs/") && location.pathname.includes("/packages/container/package/org-img"));
    await page().waitForSelector('[data-testid="pkg-install"]');
  });

  // ---------- GitHub Apps ----------
  const cdp = await page().createCDPSession();
  await cdp.send("Page.setDownloadBehavior", { behavior: "allow", downloadPath: t.WORK });

  await step("apps: register a GitHub App in developer settings", async () => {
    await goto("/settings/apps");
    await page().waitForSelector('[data-testid="apps-list"]');
    await click('[data-testid="new-app"]');
    await fill("#app-name", APP);
    await fill("#app-desc", "Registered by the e2e run");
    await fill("#app-url", "https://example.com/e2e-app");
    await fill("#app-hook", "https://example.com/hook");
    await page().select('[data-testid="perm-contents"]', "read");
    await page().select('[data-testid="perm-issues"]', "write");
    await click('[data-testid="event-issues"]');
    await shot("app-01-new-app");
    await click('[data-testid="create-app"]');
    await page().waitForSelector('[data-testid="app-created"]');
    pem = await page().$eval('[data-testid="pem"]', (e) => e.textContent);
    assert(pem.includes("PRIVATE KEY"), "PEM shown");
    appId = Number(await page().$eval('[data-testid="app-id"]', (e) => e.textContent));
    clientId = await page().$eval('[data-testid="app-client-id"]', (e) => e.textContent);
    appSlug = new URL(page().url()).pathname.split("/").pop();
    assert(appId > 0 && appSlug, `app ${appId} ${appSlug}`);
    await shot("app-02-created");
    const deadline = Date.now() + 5000;
    while (!readdirSync(t.WORK).some((f) => f.endsWith(".pem")) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 200));
    assert(readdirSync(t.WORK).some((f) => f.endsWith(".private-key.pem")), "PEM downloaded");
  });

  await step("apps: edit the app, rotate keys and client secret", async () => {
    await fill("#ag-desc", "Edited by the e2e run");
    await click('[data-testid="app-save"]');
    await waitText("Your changes have been saved.");
    await click('[data-testid="generate-key"]');
    await page().waitForFunction(() => document.querySelectorAll('[data-testid="app-key-row"]').length === 2);
    const newPem = await page().$eval('[data-testid="app-keys"] [data-testid="pem"]', (e) => e.textContent);
    assert(newPem !== pem, "a new key");
    // delete the original key: JWTs signed with it stop working
    const old = await (await page().$$('[data-testid="delete-key"]'))[0];
    await old.click();
    await page().waitForFunction(() => document.querySelectorAll('[data-testid="app-key-row"]').length === 1);
    const r1 = await raw("GET", `${BASE}/api/v3/app`, { auth: false, headers: { Authorization: `Bearer ${jwt(appId, pem)}`, Accept: "application/vnd.github+json" } });
    assert(r1.status === 401, `deleted key rejected: ${r1.status}`);
    pem = newPem;
    const r2 = await raw("GET", `${BASE}/api/v3/app`, { auth: false, headers: { Authorization: `Bearer ${jwt(appId, pem)}`, Accept: "application/vnd.github+json" } });
    assert(r2.status === 200 && r2.json.description === "Edited by the e2e run", `GET /app with new key: ${r2.status} ${r2.text}`);
    await click('[data-testid="new-client-secret"]');
    await page().waitForSelector('input[aria-label="New client secret"]');
    await shot("app-03-general");
  });

  await step("apps: install on selected repositories from the Install App tab", async () => {
    await click('[data-testid="app-tab-installations"]');
    await page().waitForSelector('[data-testid="install-on-account"]');
    await shot("app-04-install-tab");
    await click('[data-testid="install-on-account"]');
    await page().waitForSelector('[data-testid="install-form"]');
    await click('[data-testid="select-repos"]');
    await click(`[data-testid="repo-pick-${REPO}"]`);
    await shot("app-05-install-form");
    await click('[data-testid="confirm-install"]');
    await page().waitForSelector('[data-testid="installation-page"]');
    await waitText("was installed on");
    instId = Number(new URL(page().url()).pathname.split("/").pop());
    assert(instId > 0, page().url());
    await shot("app-06-installation");
  });

  const mint = async () => raw("POST", `${BASE}/api/v3/app/installations/${instId}/access_tokens`, {
    auth: false, headers: { Authorization: `Bearer ${jwt(appId, pem)}`, Accept: "application/vnd.github+json" },
  });

  await step("apps: installation token (JWT signed in node) is scoped to the installation", async () => {
    const r = await mint();
    assert(r.status === 201 && r.json.token.startsWith("ghs_"), `token: ${r.status} ${r.text}`);
    assert(r.json.permissions.contents === "read" && r.json.permissions.issues === "write" && !r.json.permissions.pull_requests, `perms ${JSON.stringify(r.json.permissions)}`);
    const h = { Authorization: `token ${r.json.token}`, Accept: "application/vnd.github+json" };
    const repos = await raw("GET", `${BASE}/api/v3/installation/repositories`, { auth: false, headers: h });
    assert(repos.json.repositories.length === 1 && repos.json.repositories[0].name === REPO, `repos ${repos.text}`);
    const inScope = await raw("GET", `${BASE}/api/v3/repos/${me}/${REPO}`, { auth: false, headers: h });
    assert(inScope.status === 200, `selected repo: ${inScope.status}`);
    const outOfScope = await raw("GET", `${BASE}/api/v3/repos/${me}/${OTHER}`, { auth: false, headers: h });
    assert(outOfScope.status === 404, `other repo: ${outOfScope.status}`);
    const issue = await raw("POST", `${BASE}/api/v3/repos/${me}/${REPO}/issues`, { auth: false, headers: { ...h, "Content-Type": "application/json" }, body: JSON.stringify({ title: "from the app" }) });
    assert(issue.status === 201 && issue.json.user.login === `${appSlug}[bot]`, `issue as bot: ${issue.status} ${issue.text}`);
  });

  await step("apps: permission update needs the installation owner's approval", async () => {
    await goto(`/settings/apps/${appSlug}/permissions`);
    await page().select('[data-testid="perm-pull_requests"]', "read");
    await click('[data-testid="save-permissions"]');
    await waitText("Permissions saved.");
    let r = await mint();
    assert(!r.json.permissions.pull_requests, "not yet accepted");
    await goto(`/settings/installations/${instId}`);
    await page().waitForSelector('[data-testid="pending-permissions"]');
    await shot("app-07-pending-permissions");
    await click('[data-testid="accept-permissions"]');
    await waitText("New permissions accepted.");
    r = await mint();
    assert(r.json.permissions.pull_requests === "read", `accepted perms ${JSON.stringify(r.json.permissions)}`);
  });

  await step("apps: suspend, unsuspend and change repository access", async () => {
    await click('[data-testid="toggle-suspend"]');
    await waitText("This installation was suspended");
    let r = await mint();
    assert(r.status === 403, `suspended mint: ${r.status}`);
    await click('[data-testid="toggle-suspend"]');
    await page().waitForFunction(() => !document.body.innerText.includes("This installation was suspended"));
    r = await mint();
    assert(r.status === 201, `unsuspended mint: ${r.status}`);
    await click('[data-testid="inst-all"]');
    await click('[data-testid="save-installation"]');
    await waitText("Repository access saved.");
    r = await mint();
    const repos = await raw("GET", `${BASE}/api/v3/installation/repositories`, { auth: false, headers: { Authorization: `token ${r.json.token}` } });
    assert(repos.json.total_count >= 2, `all repos: ${repos.json.total_count}`);
  });

  await step("apps: Settings → Applications lists the installation; public app page", async () => {
    await goto("/settings/installations");
    await page().waitForSelector('[data-testid="installation-row"]');
    await waitText(appSlug);
    await shot("app-08-installed-apps");
    await goto(`/apps/${appSlug}`);
    await page().waitForSelector('[data-testid="app-install"]');
    await waitText("Read and write");
    await shot("app-09-public-page");
    await click('[data-testid="app-install"]');
    await page().waitForSelector('[data-testid="app-install-page"]');
    await waitText("Configure");
  });

  await step("apps: device flow for the GitHub App → Authorized GitHub Apps → revoke", async () => {
    const dc = await raw("POST", `${BACKEND}/login/device/code`, { auth: false, headers: { Accept: "application/json", "Content-Type": "application/json" }, body: JSON.stringify({ client_id: clientId }) });
    assert(dc.status === 200 && dc.json.user_code, `device code: ${dc.text}`);
    await goto("/login/device");
    await fill("#device-code", dc.json.user_code);
    await click('[data-testid="device-continue"]');
    await waitText(`Authorize ${APP}`);
    await click('[data-testid="device-authorize-btn"]');
    await page().waitForSelector('[data-testid="device-approved"]');
    const tok = await raw("POST", `${BACKEND}/login/oauth/access_token`, { auth: false, headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify({ client_id: clientId, device_code: dc.json.device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" }) });
    assert(tok.json.access_token?.startsWith("ghu_"), `user-to-server token: ${tok.text}`);
    await goto("/settings/installations/authorized");
    await page().waitForSelector('[data-testid="authorization-row"]');
    await waitText(APP);
    await click('[data-testid="revoke-authorization"]');
    await waitText(`Access for ${APP} revoked.`);
    const who = await raw("GET", `${BASE}/api/v3/user`, { auth: false, headers: { Authorization: `token ${tok.json.access_token}` } });
    assert(who.status === 401, `revoked ghu_ token: ${who.status}`);
  });

  await step("apps: uninstall, make public, delete the app", async () => {
    await goto(`/settings/installations/${instId}`);
    await click('[data-testid="uninstall"]');
    await page().waitForFunction(() => location.pathname === "/settings/installations");
    const r = await mint();
    assert(r.status === 404, `mint after uninstall: ${r.status}`);
    await goto(`/settings/apps/${appSlug}/advanced`);
    await click('[data-testid="toggle-public"]');
    await waitText("Make this GitHub App private");
    await shot("app-10-advanced");
    await fill("#app-del", APP);
    await click('[data-testid="delete-app"]');
    await page().waitForSelector('[data-testid="apps-list"]');
    assert((await t.api(`/apps/${appSlug}`)).status === 404, "app deleted");
  });

  // ---------- OAuth apps ----------
  let oslug = "";
  let oclient = "";
  await step("oauth: register, edit and rotate the secret of an OAuth app", async () => {
    await goto("/settings/applications");
    await page().waitForSelector('[data-testid="oauth-apps-list"]');
    await click('[data-testid="new-oauth-app"]');
    await fill("#oa-name", OAPP);
    await fill("#oa-url", "https://example.com/oauth");
    await fill("#oa-desc", "An OAuth app from the e2e run");
    await fill("#oa-cb", "https://example.com/oauth/callback");
    await click('[data-testid="register-oauth-app"]');
    await page().waitForSelector('[data-testid="oauth-app-page"]');
    await page().waitForSelector('input[aria-label="Client secret"]');
    oslug = new URL(page().url()).pathname.split("/").pop();
    oclient = await page().$eval('input[aria-label="Client ID"]', (e) => e.value);
    await fill("#oe-name", `${OAPP} edited`);
    await click('[data-testid="update-oauth-app"]');
    await waitText("Application updated successfully.");
    await click('[data-testid="new-client-secret"]');
    await page().waitForSelector('input[aria-label="Client secret"]');
    await shot("oauth-01-app");
    await goto("/settings/applications");
    await waitText(`${OAPP} edited`);
  });

  await step("oauth: device flow approval, Authorized OAuth Apps, revoke", async () => {
    const dc = await raw("POST", `${BACKEND}/login/device/code`, { auth: false, headers: { Accept: "application/json", "Content-Type": "application/json" }, body: JSON.stringify({ client_id: oclient, scope: "repo read:user" }) });
    assert(dc.json?.user_code, `device code: ${dc.text}`);
    await goto(`/login/device`);
    await shot("oauth-02-device");
    await fill("#device-code", "ZZZZ-ZZZZ");
    await click('[data-testid="device-continue"]');
    await waitText("That code is invalid or has expired.");
    await fill("#device-code", dc.json.user_code);
    await click('[data-testid="device-continue"]');
    await waitText(`Authorize ${OAPP} edited`);
    await waitText("read:user");
    await shot("oauth-03-device-authorize");
    await click('[data-testid="device-authorize-btn"]');
    await page().waitForSelector('[data-testid="device-approved"]');
    const tok = await raw("POST", `${BACKEND}/login/oauth/access_token`, { auth: false, headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify({ client_id: oclient, device_code: dc.json.device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" }) });
    assert(tok.json.access_token, `token: ${tok.text}`);
    const who = await raw("GET", `${BASE}/api/v3/user`, { auth: false, headers: { Authorization: `token ${tok.json.access_token}` } });
    assert(who.status === 200 && who.json.login === me, `token works: ${who.status}`);
    await goto("/settings/installations/oauth");
    await page().waitForSelector('[data-testid="authorization-row"]');
    await waitText(`${OAPP} edited`);
    await shot("oauth-04-authorized");
    const rows = await page().$$('[data-testid="authorization-row"]');
    for (const r of rows) {
      if ((await r.evaluate((e) => e.innerText)).includes(`${OAPP} edited`)) { await (await r.$('[data-testid="revoke-authorization"]')).click(); break; }
    }
    await waitText(`revoked.`);
    const gone = await raw("GET", `${BASE}/api/v3/user`, { auth: false, headers: { Authorization: `token ${tok.json.access_token}` } });
    assert(gone.status === 401, `revoked token: ${gone.status}`);
  });

  await step("oauth: delete the OAuth app", async () => {
    await goto(`/settings/applications/${oslug}`);
    await click('[data-testid="delete-oauth-app"]');
    await page().waitForSelector('[data-testid="oauth-apps-list"]');
    await page().waitForFunction((n) => !document.body.innerText.includes(n), {}, `${OAPP} edited`);
  });

  // ---------- organization settings sections (shell rendered by the orgs area) ----------
  await step("org settings: private registries, GitHub Apps, packages (needs the orgs area's settings shell)", async () => {
    await goto(`/organizations/${ORG}/settings/private-registries`);
    if (!(await settingsShell())) {
      log("    SKIP org settings sections: /organizations/:org/settings/* is not rendered (orgs area shell missing)");
      return;
    }
    await page().waitForSelector('[data-testid="private-registries"]');
    await click('[data-testid="new-registry"]');
    await page().waitForSelector("#reg-type");
    await page().select("#reg-type", "npm_registry");
    await fill("#reg-url", "https://npm.example.com/");
    await fill("#reg-secret", "s3cret-token");
    await click('[data-testid="save-registry"]');
    await page().waitForSelector('[data-testid="registry-row"]');
    await waitText("NPM_REGISTRY_SECRET");
    await shot("org-01-private-registries");
    await clickText("NPM_REGISTRY_SECRET", "a");
    await page().waitForFunction(() => document.querySelector("#reg-url")?.value === "https://npm.example.com/");
    await fill("#reg-url", "https://npm2.example.com/");
    await fill("#reg-secret", "rotated-token");
    await click('[data-testid="save-registry"]');
    await waitText("Private registry updated.");
    const g = await api(`/orgs/${ORG}/private-registries/NPM_REGISTRY_SECRET`);
    assert(g.json.url === "https://npm2.example.com/", `updated url ${g.text}`);
    await click('[data-testid="delete-registry"]');
    await page().waitForSelector('[data-testid="private-registries"]');
    assert((await api(`/orgs/${ORG}/private-registries/NPM_REGISTRY_SECRET`)).status === 404, "registry deleted");
    await goto(`/organizations/${ORG}/settings/installations`);
    await page().waitForSelector('[data-testid="org-installations"]');
    await goto(`/organizations/${ORG}/settings/apps`);
    await page().waitForSelector('[data-testid="apps-list"]');
    await goto(`/organizations/${ORG}/settings/packages`);
    await page().waitForSelector('[data-testid="deleted-packages"]');
    await shot("org-02-org-packages-settings");
  });

  await step("packages: main pages have no horizontal overflow at 375px", async () => {
    const m = await t.page.browserContext().newPage();
    m.setDefaultNavigationTimeout(60000);
    await m.setViewport({ width: 375, height: 800, isMobile: true, hasTouch: true });
    const pages = [
      ["pkg-m1-profile-packages", `/${me}?tab=packages`],
      ["pkg-m2-npm-package", `/users/${me}/packages/npm/package/${NPM_NAME}`],
      ["pkg-m3-container-versions", `/users/${me}/packages/container/${IMG}/versions`],
      ["pkg-m4-package-settings", `/users/${me}/packages/container/${IMG}/settings`],
      ["pkg-m5-org-packages", `/orgs/${ORG}/packages`],
      ["pkg-m6-settings-apps", "/settings/apps/new"],
      ["pkg-m7-applications", "/settings/installations"],
      ["pkg-m8-device", "/login/device"],
      ["pkg-m9-repo-packages", `/${me}/${REPO}/packages`],
    ];
    for (const [name, p] of pages) {
      await goto(p, m);
      const over = await m.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      await shot(name, m);
      assert(over <= 1, `${p} overflows by ${over}px`);
    }
    await m.close();
  });

  assert(existsSync(t.WORK), "work dir");
}

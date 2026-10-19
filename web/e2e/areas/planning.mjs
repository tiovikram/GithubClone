// e2e flows of the planning area: labels, milestones, issue templates, issue sidebar (assignees,
// milestone, relationships, sub-issues, projects), reactions, comment edit/pin/delete, timeline,
// lock, pin, transfer, delete; PR reviewers, line/multi-line comments, suggestions, pending
// reviews, resolve, dismiss, draft <-> ready, base change, update branch; /issues and /pulls
// dashboards; Projects (table, board, fields, views, items); Discussions; 375px checks.
const tid = (id) => `[data-testid="${id}"]`;
const b64 = (s) => Buffer.from(s).toString("base64");

export async function run(t) {
  const { step, assert, RUN } = t;
  const P = () => t.page;
  const me = t.me;
  const REPO = `plan-${RUN}`;
  const REPO2 = `plan2-${RUN}`;
  const full = `${me}/${REPO}`;
  const LABEL = `p-bug-${RUN}`;
  const MS = `v1-${RUN}`;
  let p2;
  let issueNo;
  let prNo;
  const waitPath = (p, re) => p.waitForFunction((s) => new RegExp(s).test(location.pathname), { timeout: 30000 }, re.source);
  const selectByText = async (p, selector, text) => {
    const v = await p.$eval(selector, (el, txt) => {
      const o = [...el.options].find((x) => x.textContent.includes(txt));
      return o ? o.value : null;
    }, text);
    assert(v !== null, `option ${text} in ${selector}`);
    await p.select(selector, v);
  };
  /** Set an input's value the way React sees it (date inputs can't be typed into reliably). */
  const setValue = (p, selector, value) => p.$eval(selector, (el, v) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    setter.call(el, v);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }, value);
  const rest = async (path, init) => {
    const r = await t.rest(path, init);
    assert(r.status < 300, `${init?.method || "GET"} ${path}: ${r.status} ${r.text.slice(0, 300)}`);
    return r.json;
  };
  const putFile = async (repo, path, content, branch, message = `edit ${path}`) => {
    let sha;
    const cur = await t.rest(`/repos/${me}/${repo}/contents/${path}?ref=${branch}`);
    if (cur.status === 200) sha = cur.json.sha;
    return rest(`/repos/${me}/${repo}/contents/${path}`, { method: "PUT", body: { message, content: b64(content), branch, sha } });
  };

  await step("planning: setup repositories, collaborator, branch and PR", async () => {
    await t.newRepo(REPO);
    await t.newRepo(REPO2);
    await putFile(REPO, ".github/ISSUE_TEMPLATE/bug_report.md", "---\nname: Bug report\nabout: Create a report to help us improve\ntitle: \"[Bug] \"\n---\n\n**Describe the bug**\n", "main", "add template");
    await putFile(REPO, "src/app.txt", "one\ntwo\nthree\nfour\nfive\nsix\n", "main", "add app");
    const main = await rest(`/repos/${me}/${REPO}/git/ref/heads/main`);
    await rest(`/repos/${me}/${REPO}/git/refs`, { method: "POST", body: { ref: "refs/heads/feature", sha: main.object.sha } });
    await rest(`/repos/${me}/${REPO}/git/refs`, { method: "POST", body: { ref: "refs/heads/develop", sha: main.object.sha } });
    await putFile(REPO, "src/app.txt", "one\ntwo!\nthree!\nfour!\nfive\nsix\nseven\n", "feature", "change app");
    const pr = await rest(`/repos/${me}/${REPO}/pulls`, { method: "POST", body: { title: `Feature ${RUN}`, head: "feature", base: "main", body: "Please review" } });
    prNo = pr.number;
    await rest(`/repos/${me}/${REPO}`, { method: "PATCH", body: { has_discussions: true } });
    p2 = await t.page2();
    const inv = await t.rest(`/repos/${me}/${REPO}/collaborators/${t.LOGIN2}`, { method: "PUT", body: { permission: "push" } });
    assert(inv.status === 201 || inv.status === 204, `invite ${inv.status} ${inv.text}`);
    if (inv.status === 201) {
      const r = await t.api(`/user/repository_invitations/${inv.json.id}`, { method: "PATCH" }, p2);
      assert(r.status === 204, `accept invitation ${r.status} ${r.text}`);
    }
  });

  // ---------- labels ----------
  await step("planning: labels page — create, edit, delete", async () => {
    const p = P();
    await t.goto(`/${full}/issues`);
    await t.click(tid("labels-link"));
    await waitPath(p, /\/labels$/);
    await t.click(tid("new-label"));
    await t.fill('form[aria-label="Create label"] input[placeholder="Label name"]', LABEL);
    await t.fill('form[aria-label="Create label"] input[placeholder="Description (optional)"]', "Something is broken");
    await t.fill('form[aria-label="Create label"] input.mono', "#d73a4a");
    await t.click('form[aria-label="Create label"] button[type=submit]');
    await p.waitForSelector(`[data-label="${LABEL}"]`);
    await t.click(`[data-label="${LABEL}"] ::-p-text(Edit)`);
    await t.fill('form[aria-label="Save changes"] input[placeholder="Description (optional)"]', "Broken things");
    await t.click('form[aria-label="Save changes"] button[type=submit]');
    await t.waitText("Broken things");
    await rest(`/repos/${me}/${REPO}/labels`, { method: "POST", body: { name: `tmp-${RUN}`, color: "ededed" } });
    await p.reload({ waitUntil: "networkidle0" });
    await t.click(`[data-label="tmp-${RUN}"] ::-p-text(Delete)`);
    await p.waitForFunction((n) => !document.querySelector(`[data-label="${n}"]`), {}, `tmp-${RUN}`);
    const l = await t.rest(`/repos/${me}/${REPO}/labels/${LABEL}`);
    assert(l.status === 200 && l.json.color === "d73a4a" && l.json.description === "Broken things", `label ${l.text}`);
    await t.shot("planning-01-labels");
  });

  // ---------- milestones ----------
  await step("planning: milestones — create, edit, close, reopen", async () => {
    const p = P();
    await t.goto(`/${full}/issues`);
    await t.click(tid("milestones-link"));
    await waitPath(p, /\/milestones$/);
    await t.click(tid("new-milestone"));
    await waitPath(p, /\/milestones\/new$/);
    await t.fill("#ms-title", `${MS}-draft`);
    await setValue(p, "#ms-due", "2030-01-15");
    await t.fill(".md-editor textarea", "First release");
    await t.clickText("Create milestone");
    await p.waitForSelector(`[data-title="${MS}-draft"]`);
    await t.click(`[data-title="${MS}-draft"] ::-p-text(Edit)`);
    await waitPath(p, /\/edit$/);
    await t.fill("#ms-title", MS);
    await t.clickText("Save changes");
    await p.waitForSelector(`[data-title="${MS}"]`);
    await t.waitText("Due by");
    await t.click(`[data-title="${MS}"] ::-p-text(Close)`);
    await p.waitForFunction((n) => !document.querySelector(`[data-title="${n}"]`), {}, MS);
    await t.goto(`/${full}/milestones?state=closed`);
    await t.click(`[data-title="${MS}"] ::-p-text(Reopen)`);
    await p.waitForFunction((n) => !document.querySelector(`[data-title="${n}"]`), {}, MS);
    await t.goto(`/${full}/milestones`);
    await p.waitForSelector(`[data-title="${MS}"]`);
    await t.shot("planning-02-milestones");
  });

  // ---------- issue templates + new issue metadata ----------
  await step("planning: new issue from a template with labels, assignee and milestone", async () => {
    const p = P();
    await t.goto(`/${full}/issues`);
    await t.clickText("New issue", "a");
    await waitPath(p, /\/issues\/new\/choose$/);
    await t.waitText("Bug report");
    await t.shot("planning-03-template-chooser");
    await t.clickText("Get started", "a");
    await waitPath(p, /\/issues\/new$/);
    await p.waitForFunction(() => document.querySelector("#issue-title")?.value === "[Bug] ");
    const body = await p.$eval(".md-editor textarea", (e) => e.value);
    assert(body.includes("Describe the bug"), `template body: ${body}`);
    await t.fill("#issue-title", `[Bug] Crash ${RUN}`);
    await t.click(tid("new-labels"));
    await t.click(`.pl-select-item[data-key="${LABEL}"]`);
    await t.clickText("Apply");
    await t.click(tid("new-assignees"));
    await t.click(`.pl-select-item[data-key="${me}"]`);
    await t.clickText("Apply");
    await t.click(tid("new-milestone"));
    await t.click(`.pl-select-item ::-p-text(${MS})`);
    await t.clickText("Submit new issue");
    await waitPath(p, /\/issues\/\d+$/);
    issueNo = Number(p.url().split("/").pop());
    await p.waitForSelector(`${tid("sidebar-milestone")} ::-p-text(${MS})`);
    await p.waitForSelector(`${tid("sidebar-assignees")} ::-p-text(${me})`);
    await t.waitText(LABEL);
    await t.shot("planning-04-issue-created");
  });

  await step("planning: issue title edit, comments (edit, quote, pin, delete) and timeline", async () => {
    const p = P();
    await t.goto(`/${full}/issues/${issueNo}`);
    await t.clickText("Edit", ".title-row button");
    await t.fill("#title-edit", `Crash on start ${RUN}`);
    await t.clickText("Save", ".title-edit button");
    await t.waitText(`Crash on start ${RUN}`);
    for (const c of ["First comment", "Second comment"]) {
      await t.fill('form[aria-label="Add a comment"] textarea', c);
      await t.click('form[aria-label="Add a comment"] button[type=submit]');
      await t.waitText(c);
      await p.waitForFunction(() => document.querySelector('form[aria-label="Add a comment"] textarea')?.value === "");
    }
    // edit the first comment
    const first = await p.waitForSelector('[id^="issuecomment-"]');
    const cid = await first.evaluate((e) => e.id);
    await t.click(`#${cid} [aria-label="Comment actions"]`);
    await t.click(`#${cid} ::-p-text(Edit)`);
    await t.fill(`#${cid} form[aria-label="Edit comment"] textarea`, "First comment (edited)");
    await t.clickText("Update comment");
    await t.waitText("First comment (edited)");
    // pin it
    await t.click(`#${cid} [aria-label="Comment actions"]`);
    await t.click(`#${cid} ::-p-text(Pin comment)`);
    await p.waitForSelector(tid("pinned-comment-note"));
    // delete the second comment
    const ids = await p.$$eval('[id^="issuecomment-"]', (els) => els.map((e) => e.id));
    const second = ids[ids.length - 1];
    await t.click(`#${second} [aria-label="Comment actions"]`);
    await t.click(`#${second} ::-p-text(Delete)`);
    await p.waitForFunction((id) => !document.getElementById(id), {}, second);
    // quote reply prefills the comment box
    await t.click('#issue-body [aria-label="Issue body actions"]');
    await t.click("#issue-body ::-p-text(Quote reply)");
    await p.waitForFunction(() => document.querySelector('form[aria-label="Add a comment"] textarea')?.value.startsWith(">"));
    await p.waitForSelector('[data-event="renamed"]');
    await p.waitForSelector('[data-event="labeled"]');
    await p.waitForSelector('[data-event="assigned"]');
    await p.waitForSelector('[data-event="milestoned"]');
    await t.shot("planning-05-issue-timeline");
  });

  await step("planning: reactions on issue and comment", async () => {
    const p = P();
    await t.goto(`/${full}/issues/${issueNo}`);
    await t.click('#issue-body [aria-label="Add reaction"]');
    await t.click('#issue-body [data-pick="+1"]');
    await p.waitForSelector('#issue-body [data-reaction="+1"].mine');
    const cid = await p.$eval('[id^="issuecomment-"]', (e) => e.id);
    await t.click(`#${cid} [aria-label="Add reaction"]`);
    await t.click(`#${cid} [data-pick="heart"]`);
    await p.waitForSelector(`#${cid} [data-reaction="heart"].mine`);
    const r = await t.rest(`/repos/${full}/issues/${issueNo}/reactions`);
    assert(r.json.some((x) => x.content === "+1"), "reaction stored");
    await t.click('#issue-body [data-reaction="+1"]');
    await p.waitForFunction(() => !document.querySelector('#issue-body [data-reaction="+1"]'));
  });

  await step("planning: assignees and milestone editing from the sidebar", async () => {
    const p = P();
    await t.goto(`/${full}/issues/${issueNo}`);
    await t.click(tid("edit-assignees"));
    await t.click(`.pl-select-item[data-key="${t.LOGIN2}"]`);
    await t.clickText("Apply");
    await p.waitForSelector(`${tid("sidebar-assignees")} .pl-people [href="/${t.LOGIN2}"]`);
    await t.click(tid("edit-milestone"));
    await t.click(`.pl-select-item ::-p-text(${MS})`); // toggles off
    await p.waitForSelector(`${tid("sidebar-milestone")} ::-p-text(No milestone)`);
    await t.click(tid("edit-milestone"));
    await t.click(`.pl-select-item ::-p-text(${MS})`);
    await p.waitForSelector(`${tid("sidebar-milestone")} .pl-strong-link`);
    const i = await rest(`/repos/${full}/issues/${issueNo}`);
    assert(i.assignees.length === 2 && i.milestone?.title === MS, `assignees/milestone ${JSON.stringify(i.assignees.map((a) => a.login))}`);
  });

  await step("planning: close as not planned, reopen, lock and unlock, pin", async () => {
    const p = P();
    await t.goto(`/${full}/issues/${issueNo}`);
    await t.click(tid("close-reason"));
    await t.click("::-p-text(Close as not planned)");
    await t.click(tid("close-issue"));
    await p.waitForSelector(tid("reopen-issue"));
    await t.waitText("closed this as not planned");
    await t.click(tid("reopen-issue"));
    await p.waitForSelector(tid("close-issue"));
    await t.click(tid("lock-toggle"));
    await p.select("#lock-reason", "too heated");
    await t.click(".pl-dialog .btn-primary");
    await p.waitForSelector(tid("locked-badge"));
    await t.waitText("limited conversation to collaborators");
    await t.shot("planning-06-locked");
    await t.click(tid("lock-toggle"));
    await p.waitForFunction(() => !document.querySelector('[data-testid="locked-badge"]'));
    await t.click(tid("pin-toggle"));
    await p.waitForSelector('[data-testid="pin-toggle"][data-pinned="true"]');
    await t.click(tid("pin-toggle"));
    await p.waitForSelector('[data-testid="pin-toggle"][data-pinned="false"]');
  });

  let subTitle;
  await step("planning: sub-issues (create and add existing) and parent link", async () => {
    const p = P();
    const other = await rest(`/repos/${full}/issues`, { method: "POST", body: { title: `Existing child ${RUN}` } });
    await t.goto(`/${full}/issues/${issueNo}`);
    subTitle = `Sub task ${RUN}`;
    await t.click(tid("add-sub-issue"));
    await t.click("::-p-text(Create sub-issue)");
    await t.fill("#sub-title", subTitle);
    await t.click(`${tid("sub-issues")} button[type=submit]`);
    await p.waitForSelector(`${tid("sub-issues")} ::-p-text(${subTitle})`);
    await t.click(tid("add-sub-issue"));
    await t.click("::-p-text(Add existing issue)");
    await t.fill(`${tid("sub-issues")} input[placeholder^="#issue"]`, `#${other.number}`);
    await t.click(`${tid("sub-issues")} form button[type=submit]`);
    await p.waitForSelector(`${tid("sub-issues")} ::-p-text(Existing child ${RUN})`);
    await t.waitText("0 of 2");
    await t.shot("planning-07-sub-issues");
    await t.goto(`/${full}/issues/${other.number}`);
    await p.waitForSelector(`${tid("rel-parent")} ::-p-text(Crash on start ${RUN})`);
  });

  await step("planning: dependencies (blocked by / blocking)", async () => {
    const p = P();
    const blocker = await rest(`/repos/${full}/issues`, { method: "POST", body: { title: `Blocker ${RUN}` } });
    await t.goto(`/${full}/issues/${issueNo}`);
    await t.click(tid("edit-relationships"));
    await t.click("::-p-text(Mark as blocked by)");
    await t.fill(`${tid("sidebar-relationships")} input`, `#${blocker.number}`);
    await t.click(`${tid("sidebar-relationships")} button[type=submit]`);
    await p.waitForSelector(`${tid("rel-blocked-by")} ::-p-text(Blocker ${RUN})`);
    await t.goto(`/${full}/issues/${blocker.number}`);
    await p.waitForSelector(`${tid("rel-blocking")} ::-p-text(Crash on start ${RUN})`);
    await t.shot("planning-08-dependencies");
  });

  await step("planning: issue list filters, search and milestone page", async () => {
    const p = P();
    await t.goto(`/${full}/issues`);
    await t.click(tid("filter-labels"));
    await t.click(`.pl-select-item[data-key="${LABEL}"]`);
    await p.waitForFunction(() => location.search.includes("labels="));
    await p.waitForFunction(() => document.querySelectorAll('[data-testid="issue-row"]').length === 1);
    await t.waitText(`Crash on start ${RUN}`);
    await t.goto(`/${full}/issues`);
    await t.fill("#issue-search", "Blocker");
    await p.keyboard.press("Enter");
    await p.waitForFunction(() => location.search.includes("q=Blocker"));
    await t.waitText(`Blocker ${RUN}`);
    await t.goto(`/${full}/issues`);
    await t.click(tid("filter-milestones"));
    await t.click(`.pl-select-item ::-p-text(${MS})`);
    await p.waitForFunction(() => location.search.includes("milestone="));
    await t.waitText(`Crash on start ${RUN}`);
    await t.goto(`/${full}/milestones`);
    await t.click(`[data-title="${MS}"] .pl-ms-title`);
    await waitPath(p, /\/milestone\/\d+$/);
    await p.waitForSelector(tid("milestone-title"));
    await t.waitText(`Crash on start ${RUN}`);
    await t.shot("planning-09-milestone-page");
  });

  await step("planning: transfer and delete issues", async () => {
    const p = P();
    const mv = await rest(`/repos/${full}/issues`, { method: "POST", body: { title: `Move me ${RUN}` } });
    await t.goto(`/${full}/issues/${mv.number}`);
    await t.click(tid("transfer-open"));
    await p.waitForSelector(`#transfer-repo option[value="${me}/${REPO2}"]`);
    await p.select("#transfer-repo", `${me}/${REPO2}`);
    await t.click(".pl-dialog .btn-primary");
    await waitPath(p, new RegExp(`/${REPO2}/issues/\\d+$`));
    await t.waitText(`Move me ${RUN}`);
    const del = await rest(`/repos/${full}/issues`, { method: "POST", body: { title: `Delete me ${RUN}` } });
    await t.goto(`/${full}/issues/${del.number}`);
    await t.click(tid("delete-issue-open"));
    await t.click(".pl-dialog .btn-danger");
    await waitPath(p, new RegExp(`/${REPO}/issues$`));
    const gone = await t.rest(`/repos/${full}/issues/${del.number}`);
    assert(gone.status === 404 || gone.status === 410, `deleted issue ${gone.status}`);
  });

  // ---------- pull requests ----------
  await step("planning: request a reviewer and assign on a PR", async () => {
    const p = P();
    await t.goto(`/${full}/pull/${prNo}`);
    await t.click(tid("edit-reviewers"));
    await t.click(`.pl-select-item[data-key="${t.LOGIN2}"]`);
    await t.clickText("Apply");
    await p.waitForSelector(`${tid("sidebar-reviewers")} [data-reviewer="${t.LOGIN2}"]`);
    await t.click(tid("edit-assignees"));
    await t.click(`.pl-select-item[data-key="${me}"]`);
    await t.clickText("Apply");
    await p.waitForSelector(`${tid("sidebar-assignees")} ::-p-text(${me})`);
    await t.waitText(`requested a review from ${t.LOGIN2}`);
    await t.shot("planning-10-pr-reviewers");
  });

  await step("planning: pull request list filters", async () => {
    const p = P();
    await t.goto(`/${full}/pulls`);
    await t.click(tid("filter-author"));
    await t.click(`.pl-select-item[data-key="${me}"]`);
    await p.waitForFunction(() => location.search.includes("author="));
    await p.waitForSelector(`${tid("pull-row")} ::-p-text(Feature ${RUN})`);
    await t.click(tid("filter-reviews"));
    await t.click(".pl-select-item[data-key=\"approved\"]");
    await p.waitForFunction(() => location.search.includes("review=approved"));
    await t.waitText("No results matched your search.");
    await t.goto(`/${full}/pulls?review=requested`, p2);
    await p2.waitForSelector(`${tid("pull-row")} ::-p-text(Feature ${RUN})`);
  });

  await step("planning: /pulls dashboard review requests (second user) and /issues dashboard", async () => {
    await t.goto("/pulls", p2);
    await p2.click(tid("dash-tab-review-requested"));
    await p2.waitForFunction(() => document.querySelector('[data-testid="dash-query"]')?.textContent.includes("review-requested:"));
    await t.waitText(`Feature ${RUN}`, p2);
    await t.shot("planning-11-pulls-dashboard", p2);
    const p = P();
    await t.goto("/");
    await t.click('.header-nav a[href="/issues"]');
    await waitPath(p, /^\/issues$/);
    await t.waitText(`Crash on start ${RUN}`);
    await t.click(tid("dash-tab-assigned"));
    await p.waitForFunction(() => document.querySelector('[data-testid="dash-query"]')?.textContent.includes("assignee:"));
    await t.waitText(`Crash on start ${RUN}`);
    await t.click('.header-nav a[href="/pulls"]');
    await waitPath(p, /^\/pulls$/);
    await t.waitText(`Feature ${RUN}`);
    await t.shot("planning-12-issues-dashboard");
  });

  await step("planning: line comment, multi-line suggestion in a pending review, submit review", async () => {
    const p = P();
    await t.goto(`/${full}/pull/${prNo}/files`);
    const file = '[data-path="src/app.txt"]';
    await p.waitForSelector(`${file} tr[data-new="7"]`);
    // single line comment on line 7 (added)
    await p.hover(`${file} tr[data-new="7"]`);
    await t.click(`${file} tr[data-new="7"] ${tid("add-line-comment")}`);
    await t.fill('form[aria-label="Add line comment"] textarea', "Why seven?");
    await t.clickText("Add single comment");
    await p.waitForSelector(`${tid("review-thread")}[data-line="7"]`);
    // multi-line: lines 2..4 with a suggestion, as a pending review
    await p.hover(`${file} tr[data-new="2"]`);
    await t.click(`${file} tr[data-new="2"] ${tid("add-line-comment")}`);
    await p.keyboard.down("Shift");
    await p.hover(`${file} tr[data-new="4"]`);
    await t.click(`${file} tr[data-new="4"] ${tid("add-line-comment")}`);
    await p.keyboard.up("Shift");
    await t.waitText("Commenting on lines R2 to R4");
    await t.click(tid("insert-suggestion"));
    const txt = await p.$eval('form[aria-label="Add line comment"] textarea', (e) => e.value);
    assert(txt.includes("```suggestion\ntwo!\nthree!\nfour!\n```"), `suggestion template: ${txt}`);
    await t.fill('form[aria-label="Add line comment"] textarea', "Calmer please\n```suggestion\ntwo\nthree\nfour\n```");
    await t.clickText("Start a review");
    await p.waitForSelector(tid("pending-comment"));
    await t.waitText("Finish your review");
    await t.click(tid("review-open"));
    await t.fill('form[aria-label="Submit review"] textarea', "A few notes");
    await t.clickText("Submit review");
    await p.waitForSelector(`${tid("review-thread")}[data-line="4"]`);
    await p.waitForFunction(() => !document.querySelector('[data-testid="pending-comment"]'));
    const comments = await rest(`/repos/${full}/pulls/${prNo}/comments`);
    const multi = comments.find((c) => c.start_line === 2 && c.line === 4);
    assert(multi, `multi-line comment: ${JSON.stringify(comments.map((c) => [c.start_line, c.line]))}`);
    await t.shot("planning-13-pr-files-review");
  });

  await step("planning: reply to a thread, react, resolve and unresolve", async () => {
    const p = P();
    await t.goto(`/${full}/pull/${prNo}/files`);
    const th = `${tid("review-thread")}[data-line="7"]`;
    await t.click(`${th} ${tid("thread-reply")}`);
    await t.fill(`${th} form[aria-label="Reply to thread"] textarea`, "Because seven");
    await t.click(`${th} form[aria-label="Reply to thread"] button[type=submit]`);
    await p.waitForSelector(`${th} ::-p-text(Because seven)`);
    await t.click(`${th} [aria-label="Add reaction"]`);
    await t.click(`${th} [data-pick="rocket"]`);
    await p.waitForSelector(`${th} [data-reaction="rocket"].mine`);
    await t.click(`${th} ${tid("thread-resolve")}`);
    await p.waitForSelector(`${th}.pl-thread-resolved`);
    await t.waitText("marked this conversation as resolved");
    // conversation tab shows the review with its threads
    await t.goto(`/${full}/pull/${prNo}`);
    await t.waitText("A few notes");
    await p.waitForSelector(`${tid("review-entry")} ${tid("review-thread")}`);
    await p.waitForSelector(`${tid("review-thread")} ::-p-text(Resolved)`);
    await t.click(`${tid("review-thread")}[data-line="7"] .pl-thread-path`);
    await t.click(`${tid("review-thread")}[data-line="7"] ${tid("thread-resolve")}`);
    await p.waitForFunction(() => !document.querySelector('[data-line="7"].pl-thread-resolved'));
    await t.shot("planning-14-pr-conversation-threads");
  });

  await step("planning: commit a suggestion", async () => {
    const p = P();
    await t.goto(`/${full}/pull/${prNo}/files`);
    const th = `${tid("review-thread")}[data-line="4"]`;
    await p.waitForSelector(`${th} ${tid("suggestion")}`);
    await t.click(`${th} ${tid("apply-suggestion")}`);
    await t.waitText("Suggestion applied");
    const f = await rest(`/repos/${full}/contents/src/app.txt?ref=feature`);
    const text = Buffer.from(f.content, "base64").toString();
    assert(text === "one\ntwo\nthree\nfour\nfive\nsix\nseven\n", `applied: ${JSON.stringify(text)}`);
  });

  await step("planning: second user approves, owner dismisses the review", async () => {
    const p = P();
    await t.goto(`/${full}/pull/${prNo}`, p2);
    await p2.click(tid("review-open"));
    await p2.evaluate(() => [...document.querySelectorAll('input[name="review-event"]')][1].click());
    await p2.click('form[aria-label="Submit review"] button[type=submit]');
    await t.waitText("approved these changes", p2);
    await t.goto(`/${full}/pull/${prNo}`);
    await t.click(tid("dismiss-review"));
    await t.fill("#dismiss-msg", "Needs another look");
    await t.click(".pl-dialog .btn-danger");
    await p.waitForSelector('[data-testid="review-entry"][data-state="DISMISSED"]');
    await t.shot("planning-15-review-dismissed");
  });

  await step("planning: convert to draft and mark ready for review", async () => {
    const p = P();
    await t.goto(`/${full}/pull/${prNo}`);
    await t.click(tid("convert-to-draft"));
    await p.waitForSelector(tid("draft-box"));
    await t.click(tid("ready-for-review"));
    await p.waitForFunction(() => !document.querySelector('[data-testid="draft-box"]'));
    await t.waitText("marked this pull request as ready for review");
  });

  await step("planning: change the base branch and update the branch", async () => {
    const p = P();
    await t.goto(`/${full}/pull/${prNo}`);
    await t.click(tid("edit-base"));
    await p.waitForSelector('#base-branch option[value="develop"]');
    await p.select("#base-branch", "develop");
    await p.waitForFunction(() => document.querySelector('[data-testid="base-ref"]')?.textContent === "develop");
    await t.click(tid("edit-base"));
    await p.waitForSelector('#base-branch option[value="main"]');
    await p.select("#base-branch", "main");
    await p.waitForFunction(() => document.querySelector('[data-testid="base-ref"]')?.textContent === "main");
    await putFile(REPO, "docs/notes.txt", "notes\n", "main", "main moves on");
    await p.reload({ waitUntil: "networkidle0" });
    await p.waitForSelector(tid("behind-box"));
    await t.click(tid("update-branch"));
    await t.waitText("Branch updated.");
    await p.waitForFunction(() => !document.querySelector('[data-testid="behind-box"]'), { timeout: 30000 });
    const cmp = await rest(`/repos/${full}/compare/feature...main`);
    assert(cmp.ahead_by === 0, `feature behind main by ${cmp.ahead_by}`);
    await t.shot("planning-16-pr-updated");
  });

  // ---------- projects ----------
  let projectUrl;
  const PROJ = `Roadmap ${RUN}`;
  await step("planning: create a project from the profile and add items", async () => {
    const p = P();
    await t.goto(`/${me}`);
    await t.click(tid("profile-tab-projects"));
    await t.click(tid("new-project"));
    await t.fill("#project-title", PROJ);
    await t.clickText("Create project");
    await waitPath(p, /\/projects\/\d+\/views\/1$/);
    projectUrl = new URL(p.url()).pathname.replace(/\/views\/1$/, "");
    await p.waitForSelector(`${tid("project-title")} ::-p-text(${PROJ})`);
    await t.fill("#add-item", "Draft task one");
    await t.click(".pl-add-item button[type=submit]");
    await p.waitForSelector('[data-testid="project-item"][data-title="Draft task one"]');
    await t.fill("#add-item", `${full}#${issueNo}`);
    await t.click(".pl-add-item button[type=submit]");
    await p.waitForSelector(`[data-testid="project-item"][data-title="Crash on start ${RUN}"]`);
    await selectByText(p, 'select[aria-label="Status for Draft task one"]', "In Progress");
    await p.waitForFunction(() => {
      const s = document.querySelector('select[aria-label="Status for Draft task one"]');
      return s && s.options[s.selectedIndex].textContent === "In Progress";
    });
    await t.shot("planning-17-project-table");
  });

  await step("planning: project fields (single select, iteration, text) and values", async () => {
    const p = P();
    await t.goto(`${projectUrl}/settings`);
    const addField = async (name, type, extra) => {
      await t.fill("#field-name", name);
      await p.select("#field-type", type);
      if (extra) await extra();
      await t.clickText("Save field");
      await p.waitForSelector(`[data-field="${name}"]`);
    };
    await addField("Priority", "SINGLE_SELECT", () => t.fill("#field-options", "Low, High"));
    await addField("Sprint", "ITERATION", () => t.fill("#field-duration", "7"));
    await addField("Notes", "TEXT");
    await t.shot("planning-18-project-settings");
    await t.goto(`${projectUrl}/views/1`);
    await selectByText(p, 'select[aria-label="Priority for Draft task one"]', "High");
    await p.waitForFunction(() => {
      const s = document.querySelector('select[aria-label="Priority for Draft task one"]');
      return s && s.options[s.selectedIndex].textContent === "High";
    });
    await selectByText(p, 'select[aria-label="Sprint for Draft task one"]', "(");
    await t.click('[aria-label="Edit Notes for Draft task one"]');
    await t.fill('input[aria-label="Notes for Draft task one"]', "remember the milk");
    await p.keyboard.press("Enter");
    await t.waitText("remember the milk");
  });

  await step("planning: board view, move a card, edit a draft, archive", async () => {
    const p = P();
    await t.goto(`${projectUrl}/views/1`);
    await t.click(tid("new-view"));
    await t.click(".pl-popover ::-p-text(Board)");
    await waitPath(p, /\/views\/2$/);
    await p.waitForSelector(tid("project-board"));
    const card = '[data-testid="board-card"][data-title="Draft task one"]';
    await p.waitForSelector(`[data-column="In Progress"] ${card}`);
    await selectByText(p, `${card} select`, "Done");
    await p.waitForSelector(`[data-column="Done"] ${card}`);
    await t.shot("planning-19-project-board");
    await t.click(`${card} .pl-item-title`);
    await t.click(tid("edit-draft"));
    await t.fill("#draft-title", "Draft task renamed");
    await t.clickText("Save", ".pl-dialog button");
    await t.click('.pl-dialog [aria-label="Close"]');
    await p.waitForSelector('[data-testid="board-card"][data-title="Draft task renamed"]');
    await t.goto(`${projectUrl}/views/1`);
    await t.click('[aria-label="Actions for Draft task renamed"]');
    await t.click(".pl-popover ::-p-text(Archive)");
    await p.waitForFunction(() => !document.querySelector('[data-title="Draft task renamed"]'));
    await t.click(".pl-view-bar input[type=checkbox]");
    await p.waitForSelector('[data-testid="project-item"][data-title="Draft task renamed"]');
  });

  await step("planning: issue sidebar projects, repo Projects tab link, close and delete a project", async () => {
    const p = P();
    await t.goto(`/${full}/issues/${issueNo}`);
    await p.waitForSelector(`${tid("sidebar-projects")} ::-p-text(${PROJ})`);
    await t.goto(`/${full}`);
    await t.click(tid("repo-tab-projects"));
    await waitPath(p, /\/projects$/);
    await t.click(tid("link-project"));
    await t.click(`.pl-popover ::-p-text(${PROJ})`);
    await p.waitForSelector(`[data-testid="project-row"][data-title="${PROJ}"]`);
    await t.shot("planning-20-repo-projects");
    // a second project, created from the repository, closed and deleted
    await t.click(tid("new-project"));
    await t.fill("#project-title", `Temp ${RUN}`);
    await t.clickText("Create project");
    await waitPath(p, /\/projects\/\d+\/views\/1$/);
    await t.click(tid("project-settings"));
    await t.click(tid("close-project"));
    await t.waitText("Project closed.");
    await t.click(tid("delete-project"));
    await waitPath(p, /\/users\/[^/]+\/projects$/);
    await p.waitForFunction((n) => !document.querySelector(`[data-title="${n}"]`), {}, `Temp ${RUN}`);
  });

  // ---------- discussions ----------
  await step("planning: discussions — create, comment, reply, answer, close, lock, delete", async () => {
    const p = P();
    await t.goto(`/${full}`);
    await t.click(tid("repo-tab-discussions"));
    await waitPath(p, /\/discussions$/);
    await t.click(tid("new-discussion"));
    await t.click(tid("choose-q-a"));
    await t.fill("#disc-title", `How do I ${RUN}?`);
    await t.fill(".md-editor textarea", "Asking for a friend");
    await t.clickText("Start discussion");
    await waitPath(p, /\/discussions\/\d+$/);
    const url = new URL(p.url()).pathname;
    await t.waitText(`How do I ${RUN}?`);
    await t.fill('form[aria-label="Add a comment to the discussion"] textarea', "Try turning it off and on");
    await t.click('form[aria-label="Add a comment to the discussion"] button[type=submit]');
    await p.waitForSelector(`${tid("discussion-comment")} ::-p-text(Try turning it off and on)`);
    await t.click(tid("discussion-reply-open"));
    await t.fill('form[aria-label="Reply"] textarea', "Worked, thanks");
    await t.click('form[aria-label="Reply"] button[type=submit]');
    await p.waitForSelector(`${tid("discussion-reply")} ::-p-text(Worked, thanks)`);
    await t.click(tid("mark-answer"));
    await p.waitForSelector(tid("unmark-answer"));
    await t.shot("planning-21-discussion");
    await t.click(tid("close-discussion"));
    await t.click(".pl-popover ::-p-text(Close as resolved)");
    await p.waitForSelector(tid("reopen-discussion"));
    await t.click(tid("reopen-discussion"));
    await p.waitForSelector(tid("close-discussion"));
    await t.click(tid("lock-discussion"));
    await p.waitForSelector(tid("discussion-locked"));
    await t.click(tid("lock-discussion"));
    await p.waitForFunction(() => !document.querySelector('[data-testid="discussion-locked"]'));
    await t.goto(`/${full}/discussions`);
    await p.waitForSelector(`[data-testid="discussion-row"][data-title="How do I ${RUN}?"]`);
    await t.goto(url);
    await t.click(tid("delete-discussion"));
    await waitPath(p, /\/discussions$/);
    await p.waitForFunction((n) => !document.querySelector(`[data-title="${n}"]`), {}, `How do I ${RUN}?`);
  });

  // ---------- permissions ----------
  await step("planning: non-collaborators get read-only planning pages", async () => {
    const other = await rest(`/repos/${me}/${REPO2}/issues`, { method: "POST", body: { title: `Read only ${RUN}` } });
    await t.goto(`/${me}/${REPO2}/labels`, p2);
    await t.waitText("labels", p2);
    assert(!(await p2.$(tid("new-label"))), "no New label for non-collaborator");
    await t.goto(`/${me}/${REPO2}/milestones`, p2);
    assert(!(await p2.$(tid("new-milestone"))), "no New milestone for non-collaborator");
    await t.goto(`/${me}/${REPO2}/issues/${other.number}`, p2);
    await t.waitText(`Read only ${RUN}`, p2);
    assert(!(await p2.$(tid("edit-assignees"))) && !(await p2.$(tid("lock-toggle"))), "no sidebar editing for non-collaborator");
    await t.goto(`${projectUrl}/settings`, p2);
    await p2.waitForFunction(() => !location.pathname.endsWith("/settings") || /not found|not the page/i.test(document.body.innerText));
    assert(!(await p2.$('form[aria-label="Project settings"]')), "no project settings for others");
  });

  await step("planning: direct URLs render (hard reload)", async () => {
    const p = P();
    for (const [path, text] of [
      ["/issues", "Created"], ["/pulls", "Review requests"], [`/users/${me}/projects`, "Projects"],
      [`${projectUrl}`, PROJ], [`/${full}/labels`, LABEL], [`/${full}/milestones`, MS], [`/${full}/projects`, PROJ],
      [`/${full}/discussions`, "Categories"],
    ]) {
      await t.goto(path);
      await t.waitText(text);
    }
    void p;
  });

  await step("planning: 375px pages have no horizontal overflow", async () => {
    const m = await t.page.browserContext().newPage();
    m.setDefaultNavigationTimeout(60000);
    await m.setViewport({ width: 375, height: 800, isMobile: true, hasTouch: true });
    const pages = [
      ["planning-m-issues", `/${full}/issues`],
      ["planning-m-issue", `/${full}/issues/${issueNo}`],
      ["planning-m-labels", `/${full}/labels`],
      ["planning-m-milestones", `/${full}/milestones`],
      ["planning-m-pr", `/${full}/pull/${prNo}`],
      ["planning-m-pr-files", `/${full}/pull/${prNo}/files`],
      ["planning-m-dashboard", "/issues"],
      ["planning-m-pulls", `/${full}/pulls`],
      ["planning-m-project", `${projectUrl}/views/1`],
      ["planning-m-board", `${projectUrl}/views/2`],
      ["planning-m-projects", `/users/${me}/projects`],
      ["planning-m-discussions", `/${full}/discussions`],
    ];
    const bad = [];
    for (const [name, path] of pages) {
      await t.goto(path, m);
      const over = await m.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      await t.shot(name, m);
      if (over > 1) bad.push(`${path} overflows by ${over}px`);
    }
    await m.close();
    assert(!bad.length, bad.join("; "));
  });
}

#!/usr/bin/env node
/* Airplane-manual long-text Web UI E2E fallback for platforms where Python
 * Playwright packages are unavailable or version-mismatched.
 */
const fs = require("fs");
const path = require("path");
const { chromium } = require("playwright");

const TERMINAL_LOCAL_SCHEDULER = new Set([
  "done",
  "completed",
  "complete",
  "success",
  "succeeded",
  "error",
  "failed",
  "failure",
  "canceled",
  "cancelled",
  "expired",
]);
const FAILED_LOCAL_SCHEDULER = new Set(["error", "failed", "failure", "canceled", "cancelled", "expired"]);

function args() {
  const out = {
    manifest: process.env.ATTUNE_LONGTEXT_MANIFEST || "tests/e2e/airplane_manual_longtext_cases.json",
    baseUrl: process.env.ATTUNE_BASE_URL || "http://localhost:18905",
    profile: process.env.ATTUNE_LONGTEXT_PROFILE || "local_scheduler_comprehensive",
    queryId: process.env.ATTUNE_LONGTEXT_UI_QUERY_ID || "",
    password: process.env.ATTUNE_E2E_PASSWORD || process.env.ATTUNE_VAULT_PW || "e2e-pass-2026",
    token: process.env.ATTUNE_TOKEN || "",
    headless: !["0", "false", "no"].includes(String(process.env.ATTUNE_HEADLESS || "1").toLowerCase()),
    executablePath: process.env.ATTUNE_PLAYWRIGHT_EXECUTABLE || "",
    timeoutMs: Number(process.env.ATTUNE_LONGTEXT_UI_TIMEOUT_MS || "120000"),
    screenshotDir: process.env.ATTUNE_LONGTEXT_UI_SHOTS || "docs/screenshots/airplane-longtext-ui",
  };
  for (let i = 2; i < process.argv.length; i += 1) {
    const key = process.argv[i];
    const value = process.argv[i + 1];
    if (!key.startsWith("--")) continue;
    i += 1;
    if (key === "--manifest") out.manifest = value;
    else if (key === "--base-url") out.baseUrl = value;
    else if (key === "--profile") out.profile = value;
    else if (key === "--query-id") out.queryId = value;
    else if (key === "--password") out.password = value;
    else if (key === "--token") out.token = value;
    else if (key === "--executable-path") out.executablePath = value;
    else if (key === "--timeout-ms") out.timeoutMs = Number(value);
    else if (key === "--screenshot-dir") out.screenshotDir = value;
  }
  out.baseUrl = out.baseUrl.replace(/\/+$/, "");
  return out;
}

function loadJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function profileDocIds(manifest, profile) {
  if (profile === "all") return new Set((manifest.documents || []).map((doc) => doc.id));
  const profiles = manifest.selection && manifest.selection.profiles ? manifest.selection.profiles : {};
  if (!profiles[profile]) {
    throw new Error(`unknown profile ${profile}`);
  }
  return new Set(profiles[profile].documents || []);
}

function selectQuery(manifest, profile, queryId) {
  const docIds = profileDocIds(manifest, profile);
  const queries = (manifest.queries || []).filter((query) =>
    (query.acceptable_hits || []).some((hit) => docIds.has(hit)),
  );
  if (queryId) {
    const found = queries.find((query) => query.id === queryId);
    if (!found) throw new Error(`query ${queryId} does not apply to profile ${profile}`);
    return found;
  }
  const preferred = (manifest.web_e2e && manifest.web_e2e.default_query_id) || "a320_qrh_abnormal";
  return queries.find((query) => query.id === preferred) || queries[0];
}

async function requestJson(opts, method, apiPath, body, token = opts.token, allowStatuses = new Set()) {
  const headers = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (token) headers.Authorization = `Bearer ${token}`;
  const response = await fetch(`${opts.baseUrl}${apiPath}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let data = {};
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = { raw: text };
    }
  }
  if (!response.ok && !allowStatuses.has(response.status)) {
    const code = data && typeof data === "object" ? data.code : undefined;
    const retryable = data && typeof data === "object" ? data.retryable : undefined;
    const mayDegrade = data && typeof data === "object" ? data.may_degrade : undefined;
    throw new Error(
      `${method} ${apiPath} failed HTTP ${response.status} code=${code} retryable=${retryable} may_degrade=${mayDegrade}: ${text.slice(0, 300)}`,
    );
  }
  return data;
}

async function ensureToken(opts) {
  if (opts.token) return opts.token;
  await requestJson(opts, "POST", "/api/v1/vault/setup", { password: opts.password }, "", new Set([400, 409]));
  const unlocked = await requestJson(opts, "POST", "/api/v1/vault/unlock", { password: opts.password }, "");
  if (!unlocked.token) throw new Error("vault unlock did not return a token");
  opts.token = unlocked.token;
  return opts.token;
}

async function ensureWizardComplete(opts, token) {
  await requestJson(
    opts,
    "PATCH",
    "/api/v1/settings",
    { wizard: { complete: true, current_step: 5 } },
    token,
    new Set([403]),
  );
}

function outputText(value) {
  if (!value) return "";
  if (typeof value === "string") return value;
  for (const key of ["answer", "text", "content", "response", "summary", "output"]) {
    if (typeof value[key] === "string" && value[key].trim()) return value[key];
  }
  if (Array.isArray(value.choices) && value.choices.length > 0) {
    const first = value.choices[0];
    if (first && typeof first.text === "string") return first.text;
    if (first && first.message && typeof first.message.content === "string") return first.message.content;
  }
  return "";
}

function schedulerStatus(value) {
  return String((value && (value.status || value.state)) || "").toLowerCase();
}

async function maybePollLocalScheduler(opts, response, token) {
  const scheduler = response.local_scheduler;
  if (!scheduler || typeof scheduler !== "object") return { content: outputText(response), job: null };
  const jobId = scheduler.job_id;
  if (!jobId || TERMINAL_LOCAL_SCHEDULER.has(schedulerStatus(scheduler))) {
    return { content: outputText(response), job: scheduler };
  }
  const deadline = Date.now() + opts.timeoutMs;
  let content = outputText(response);
  let lastJob = null;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 1500));
    const data = await requestJson(opts, "GET", `/api/v1/chat/local-scheduler/jobs/${encodeURIComponent(jobId)}`);
    const job = data.job && typeof data.job === "object" ? data.job : data;
    lastJob = job;
    if (TERMINAL_LOCAL_SCHEDULER.has(schedulerStatus(job))) {
      const outputs = job.outputs || job;
      content = outputText(outputs) || content;
      break;
    }
  }
  return { content, job: lastJob };
}

function normalizeCompact(text) {
  return String(text).toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");
}

function normalizeSpaced(text) {
  return String(text).toLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").replace(/\s+/g, " ").trim();
}

function expectedTermHit(content, terms) {
  if (!terms || terms.length === 0) return Boolean(String(content).trim());
  const raw = String(content).toLowerCase();
  const compact = normalizeCompact(content);
  const spaced = normalizeSpaced(content);
  return terms.some((term) => {
    const needle = String(term).toLowerCase().trim();
    return needle && (raw.includes(needle) || compact.includes(normalizeCompact(needle)) || spaced.includes(normalizeSpaced(needle)));
  });
}

function citationHit(citations, hits, files) {
  const haystack = JSON.stringify(citations || []).toLowerCase();
  const needles = [...(hits || [])];
  for (const file of files || []) {
    needles.push(file);
    needles.push(path.basename(file, path.extname(file)));
  }
  return needles.some((needle) => needle && haystack.includes(String(needle).toLowerCase()));
}

async function clickButton(page, names, timeout = 5000) {
  for (const name of names) {
    const locator = page.locator(`button[aria-label="${name}"]`).first();
    if ((await locator.count()) > 0) {
      await locator.click({ timeout });
      return;
    }
  }
  for (const name of names) {
    try {
      await page.getByRole("button", { name }).first().click({ timeout });
      return;
    } catch {
      // Try the next label.
    }
  }
  throw new Error(`button not found: ${names.join(" / ")}`);
}

async function screenshot(page, opts, name) {
  fs.mkdirSync(opts.screenshotDir, { recursive: true });
  await page.screenshot({ path: path.join(opts.screenshotDir, `${name}.png`), fullPage: false }).catch((err) => {
    console.log(`[ui-js] screenshot ${name} failed: ${err.message}`);
  });
}

async function visibleTextAny(page, labels) {
  for (const label of labels) {
    if ((await page.getByText(label, { exact: true }).count()) > 0) {
      if (await page.getByText(label, { exact: true }).first().isVisible()) return true;
    }
  }
  return false;
}

async function waitTextAny(page, labels, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await visibleTextAny(page, labels)) return;
    await page.waitForTimeout(250);
  }
  throw new Error(`timed out waiting for visible text: ${labels.join(" / ")}`);
}

async function waitMain(page, opts) {
  await page.goto(opts.baseUrl, { waitUntil: "networkidle", timeout: opts.timeoutMs });
  if ((await page.getByRole("button", { name: "解锁" }).count()) > 0) {
    await page.getByLabel("主密码").fill(opts.password, { timeout: 10000 });
    await page.getByRole("button", { name: "解锁" }).click();
  }
  await page.locator('button[aria-label="新对话"], button[aria-label="New chat"]').first().waitFor({
    state: "visible",
    timeout: opts.timeoutMs,
  });
  for (const name of ["我知道了", "知道了", "Got it", "OK"]) {
    try {
      await page.getByRole("button", { name }).first().click({ timeout: 1500 });
      break;
    } catch {
      // No first-run modal.
    }
  }
}

async function verifyItemVisible(page, query, opts) {
  await clickButton(page, ["条目", "Items"]);
  const firstFile = (query.acceptable_files || [""])[0];
  const needle = path.basename(firstFile, path.extname(firstFile)) || (query.expect_any || [""])[0];
  await page.locator('input[type="text"]').first().fill(needle, { timeout: 10000 });
  await page.waitForTimeout(800);
  await page.getByText(needle, { exact: false }).first().waitFor({ state: "visible", timeout: 30000 });
  await screenshot(page, opts, "01-items-indexed-js");
  return needle;
}

async function sendChatAndCapture(page, query, opts) {
  await clickButton(page, ["新对话", "New chat"]);
  let textbox = page.getByLabel("对话输入框");
  if ((await textbox.count()) === 0) textbox = page.getByLabel("Chat input");
  await textbox.fill(query.query, { timeout: 10000 });
  const start = performance.now();
  const responsePromise = page.waitForResponse(
    (response) => response.request().method() === "POST" && response.url().replace(/\/+$/, "").endsWith("/api/v1/chat"),
    { timeout: opts.timeoutMs },
  );
  try {
    await page.getByRole("button", { name: "发送消息" }).click({ timeout: 5000 });
  } catch {
    await page.getByRole("button", { name: "Send message" }).click({ timeout: 5000 });
  }
  const response = await responsePromise;
  const elapsedMs = performance.now() - start;
  if (response.status() >= 400) {
    const text = await response.text();
    let data = {};
    try {
      data = text ? JSON.parse(text) : {};
    } catch {
      data = { raw: text };
    }
    throw new Error(
      `chat UI request failed HTTP ${response.status()} code=${data.code} retryable=${data.retryable} may_degrade=${data.may_degrade}: ${text.slice(0, 500)}`,
    );
  }
  return { elapsedMs, data: await response.json() };
}

async function main() {
  const opts = args();
  const manifest = loadJson(opts.manifest);
  const query = selectQuery(manifest, opts.profile, opts.queryId);
  const targetMs =
    (((manifest.evaluation_targets || {}).rag_answer || {}).local_scheduler_30b_p95_latency_ms_max) || 10000;
  const token = await ensureToken(opts);
  await ensureWizardComplete(opts, token);

  const launchOptions = { headless: opts.headless };
  if (opts.executablePath) launchOptions.executablePath = opts.executablePath;
  const browser = await chromium.launch(launchOptions);
  const context = await browser.newContext({ locale: "zh-CN", viewport: { width: 1440, height: 900 } });
  await context.addInitScript((t) => sessionStorage.setItem("attune_token", t), token);
  const page = await context.newPage();
  const consoleErrors = [];
  page.on("console", (msg) => {
    if (msg.type() === "error" && !msg.text().includes("favicon") && !msg.text().includes("ws/scan-progress")) {
      consoleErrors.push(msg.text());
    }
  });

  try {
    console.log("=== airplane manual longtext Web UI E2E JS ===");
    console.log(`[ui-js] profile=${opts.profile} query=${query.id}`);
    await waitMain(page, opts);
    await screenshot(page, opts, "00-main-js");
    const itemNeedle = await verifyItemVisible(page, query, opts);
    console.log(`[ui-js] indexed item visible: ${itemNeedle}`);

    const turnStart = performance.now();
    const { data: response } = await sendChatAndCapture(page, query, opts);
    const { content: finalContent, job } = await maybePollLocalScheduler(opts, response, token);
    if (job && FAILED_LOCAL_SCHEDULER.has(schedulerStatus(job))) {
      throw new Error(
        `local scheduler job ended with ${schedulerStatus(job)}: ${JSON.stringify(job.error || job).slice(0, 500)}`,
      );
    }
    const probe = finalContent.trim().slice(0, 40);
    if (probe) await page.getByText(probe, { exact: false }).first().waitFor({ state: "visible", timeout: opts.timeoutMs });
    if (response.local_scheduler && typeof response.local_scheduler === "object") {
      await waitTextAny(page, ["本地调度器", "Local scheduler"], Math.min(opts.timeoutMs, 30000));
    }
    if (Array.isArray(response.citations) && response.citations.length > 0) {
      await waitTextAny(page, ["📎 引用", "📎 Citations"], Math.min(opts.timeoutMs, 30000));
    }
    const totalMs = performance.now() - turnStart;
    await screenshot(page, opts, "02-chat-answer-js");

    const citations = Array.isArray(response.citations) ? response.citations : [];
    const checks = {
      answer_term_hit: expectedTermHit(finalContent, query.expect_any || []),
      citation_hit: citationHit(citations, query.acceptable_hits || [], query.acceptable_files || []),
      citation_visible: citations.length === 0 || await visibleTextAny(page, ["📎 引用", "📎 Citations"]),
      latency_target: totalMs <= Number(targetMs),
    };
    if (response.local_scheduler && typeof response.local_scheduler === "object") {
      checks.local_scheduler_status_visible = await visibleTextAny(page, ["本地调度器", "Local scheduler"]);
    }
    if (consoleErrors.length > 0) checks.console_errors = false;
    console.log(JSON.stringify({ checks, latency_ms: totalMs, target_ms: targetMs }, null, 2));
    const failed = Object.entries(checks)
      .filter(([, ok]) => !ok)
      .map(([name]) => name);
    if (failed.length > 0) throw new Error(`UI checks failed: ${failed.join(", ")}`);
  } finally {
    await context.close();
    await browser.close();
  }

  console.log(`=== airplane manual longtext Web UI E2E JS PASS query=${query.id} ===`);
}

main().catch((err) => {
  console.log(`=== airplane manual longtext Web UI E2E JS FAIL: ${err.message} ===`);
  process.exit(1);
});

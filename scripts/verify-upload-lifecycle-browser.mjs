import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";

const output = mkdtempSync(join(tmpdir(), "axonyx-upload-lifecycle-"));
const port = Number(process.env.AXONYX_UPLOAD_E2E_PORT ?? 4175);
const origin = `http://127.0.0.1:${port}`;
const payload = Buffer.alloc(256 * 1024, "a");
const server = spawn(
  "cargo",
  ["run", "-p", "axonyx-runtime", "--features", "axum,storage", "--example", "upload_lifecycle_fixture"],
  {
    env: {
      ...process.env,
      AXONYX_E2E_OUTPUT: output,
      AXONYX_E2E_PORT: String(port),
    },
    stdio: ["ignore", "pipe", "pipe"],
  },
);

let serverOutput = "";
server.stdout.on("data", (chunk) => {
  serverOutput += chunk;
});
server.stderr.on("data", (chunk) => {
  serverOutput += chunk;
});

async function waitForServer(timeoutMs = 60_000) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (server.exitCode !== null) {
      throw new Error(`upload fixture exited early\n${serverOutput}`);
    }
    try {
      const response = await fetch(origin);
      if (response.ok) return;
    } catch (_error) {
      // Cargo may still be compiling the fixture.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`upload fixture did not start at ${origin}\n${serverOutput}`);
}

function storedObjectPaths(root) {
  const entries = readdirSync(root, { withFileTypes: true });
  return entries.flatMap((entry) => {
    const path = join(root, entry.name);
    return entry.isDirectory() ? storedObjectPaths(path) : [path];
  });
}

async function stopServer() {
  if (server.exitCode !== null) return;
  try {
    await fetch(`${origin}/__axonyx/e2e-shutdown`);
  } catch (_error) {
    // A failed test may stop before the fixture begins accepting requests.
  }
  await Promise.race([
    new Promise((resolve) => server.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ]);
  if (server.exitCode !== null) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/pid", String(server.pid), "/T", "/F"], { stdio: "ignore" });
  } else {
    server.kill("SIGKILL");
  }
  await new Promise((resolve) => server.once("exit", resolve));
}

let browser;
try {
  await waitForServer();
  browser = await chromium.launch({ channel: process.env.AXONYX_E2E_BROWSER_CHANNEL ?? "chrome" });
  const page = await browser.newPage();
  const consoleErrors = [];
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => consoleErrors.push(error.message));

  await page.goto(origin, { waitUntil: "domcontentloaded" });
  await page.evaluate(() => {
    window.__uploadLifecycle = [];
    for (const name of [
      "axonyx:upload-start",
      "axonyx:upload-progress",
      "axonyx:upload-complete",
      "axonyx:upload-error",
      "axonyx:action-complete",
      "axonyx:action-error",
    ]) {
      window.addEventListener(name, (event) => {
        const progress = event.detail.form.querySelector("[data-ax-action-progress]");
        window.__uploadLifecycle.push({
          name,
          hidden: progress.hidden,
          state: event.detail.form.dataset.axUploadState ?? null,
          loaded: event.detail.loaded ?? null,
          total: event.detail.total ?? null,
          percent: event.detail.percent ?? null,
        });
      });
    }
  });

  await page.getByLabel("Fixture file").setInputFiles({
    name: "browser-fixture.txt",
    mimeType: "text/plain",
    buffer: payload,
  });
  const [response] = await Promise.all([
    page.waitForResponse((candidate) => candidate.url().includes("/__axonyx/action?")),
    page.getByRole("button", { name: "Upload fixture", exact: true }).click(),
  ]);
  await page.waitForFunction(() => document.querySelector("form")?.dataset.axActionState === "complete");

  if (response.status() !== 200) throw new Error(`expected upload HTTP 200, got ${response.status()}`);
  const action = await response.json();
  if (!action.ok || action.value?.storage !== "media") {
    throw new Error(`unexpected upload action payload: ${JSON.stringify(action)}`);
  }
  if (action.value.file_name !== "browser-fixture.txt" || action.value.size !== payload.length) {
    throw new Error(`unexpected FileRef metadata: ${JSON.stringify(action.value)}`);
  }

  const result = await page.evaluate(() => {
    const form = document.querySelector("form");
    const progress = form.querySelector("[data-ax-action-progress]");
    return {
      events: window.__uploadLifecycle,
      actionState: form.dataset.axActionState,
      uploadState: form.dataset.axUploadState,
      loaded: Number(form.dataset.axUploadLoaded),
      total: Number(form.dataset.axUploadTotal),
      percent: Number(form.dataset.axUploadPercent),
      progressHidden: progress.hidden,
      progressValue: progress.value,
      progressAriaHidden: progress.getAttribute("aria-hidden"),
    };
  });

  const names = result.events.map((event) => event.name);
  const start = names.indexOf("axonyx:upload-start");
  const progress = names.indexOf("axonyx:upload-progress");
  const uploadComplete = names.indexOf("axonyx:upload-complete");
  const actionComplete = names.indexOf("axonyx:action-complete");
  if (!(start >= 0 && progress > start && uploadComplete > progress && actionComplete > uploadComplete)) {
    throw new Error(`unexpected lifecycle order: ${names.join(" -> ")}`);
  }
  if (names.includes("axonyx:upload-error") || names.includes("axonyx:action-error")) {
    throw new Error(`successful upload emitted an error event: ${names.join(" -> ")}`);
  }
  if (result.events[progress].hidden || result.events[uploadComplete].hidden) {
    throw new Error("ActionProgress was hidden while browser upload events were dispatched");
  }
  if (result.actionState !== "complete" || result.uploadState !== "complete") {
    throw new Error(`unexpected final states: ${JSON.stringify(result)}`);
  }
  if (result.percent !== 100 || result.progressValue !== 100 || result.loaded !== result.total) {
    throw new Error(`upload did not finish at 100 percent: ${JSON.stringify(result)}`);
  }
  if (!result.progressHidden || result.progressAriaHidden !== "true") {
    throw new Error("ActionProgress was not hidden after server action completion");
  }

  const objectRoot = join(output, "storage", "media", "objects");
  const objects = storedObjectPaths(objectRoot);
  if (objects.length !== 1) throw new Error(`expected one stored object, found ${objects.length}`);
  const stored = readFileSync(objects[0]);
  if (!stored.equals(payload)) throw new Error("stored object bytes differ from browser payload");
  const expectedId = createHash("sha256").update(payload).digest("hex");
  if (action.value.id !== expectedId) throw new Error("FileRef id does not match payload SHA-256");
  if (consoleErrors.length !== 0) throw new Error(`browser console errors: ${consoleErrors.join(" | ")}`);

  console.log("Upload lifecycle Chromium E2E passed.");
} finally {
  await browser?.close();
  await stopServer();
  try {
    rmSync(output, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  } catch (error) {
    console.warn(`Could not remove upload fixture directory ${output}: ${error.message}`);
  }
}

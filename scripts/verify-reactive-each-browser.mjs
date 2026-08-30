import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";

const output = mkdtempSync(join(tmpdir(), "axonyx-reactive-each-"));
const build = spawnSync("cargo", ["run", "-p", "axonyx-runtime", "--example", "reactive_each_fixture"], {
  encoding: "utf8",
  env: { ...process.env, AXONYX_E2E_OUTPUT: output },
});

if (build.status !== 0) {
  process.stdout.write(build.stdout ?? "");
  process.stderr.write(build.stderr ?? "");
  process.exit(build.status ?? 1);
}

const server = createServer((request, response) => {
  const wasm = request.url === "/_ax/runtime/axonyx-state-v2.wasm";
  const path = wasm ? join(output, "_ax", "runtime", "axonyx-state-v2.wasm") : join(output, "index.html");
  response.writeHead(200, { "Content-Type": wasm ? "application/wasm" : "text/html; charset=utf-8" });
  response.end(readFileSync(path));
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const address = server.address();
const origin = `http://127.0.0.1:${address.port}`;
let browser;

try {
  browser = await chromium.launch({ channel: process.env.AXONYX_E2E_BROWSER_CHANNEL ?? "chrome" });
  const page = await browser.newPage();
  const consoleErrors = [];
  const localRequests = [];
  let captureRequests = false;
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => consoleErrors.push(error.message));
  page.on("request", (request) => {
    if (captureRequests) localRequests.push(request.url());
  });

  await page.goto(origin, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.__axonyx?.state?.runtime() === "wasm");
  const list = page.locator('ax-state-each[data-ax-each-render-status="ready"]');
  const firstItem = list.locator('ax-each-item[data-ax-each-key="first"]');
  const firstHandle = await firstItem.elementHandle();
  await firstItem.locator("input").focus();
  captureRequests = true;

  await page.locator("#update").evaluate((button) => button.click());
  await page.waitForFunction(() => document.querySelector('[data-post-id="first"] span')?.textContent === "Alpha updated");
  const sameNode = await firstItem.evaluate((node, original) => node === original, firstHandle);
  if (!sameNode) throw new Error("same-key update replaced the owned DOM boundary");
  if (!(await firstItem.locator("input").evaluate((node) => node === document.activeElement))) {
    throw new Error("same-key update did not preserve focus");
  }
  const secondInput = list.locator('ax-each-item[data-ax-each-key="second"] input');
  if (await secondInput.isDisabled()) throw new Error("boolean attribute update was not applied");

  await page.locator("#insert").click();
  const inserted = list.locator('ax-each-item[data-ax-each-key="third"]');
  await inserted.waitFor();
  if ((await inserted.locator("strong").count()) !== 0) throw new Error("inserted text was interpreted as HTML");
  if ((await inserted.locator("span").textContent()) !== "<strong>Literal</strong>") {
    throw new Error("inserted text was not preserved literally");
  }

  await page.locator("#reorder").click();
  const reordered = await list.locator(":scope > ax-each-item").evaluateAll((nodes) => nodes.map((node) => node.dataset.axEachKey));
  if (reordered.join(",") !== "third,first,second") throw new Error(`unexpected reorder: ${reordered.join(",")}`);
  if (!(await firstItem.evaluate((node, original) => node === original, firstHandle))) {
    throw new Error("reorder replaced a surviving DOM boundary");
  }

  await page.locator("#remove").click();
  if ((await list.locator('ax-each-item[data-ax-each-key="second"]').count()) !== 0) {
    throw new Error("removed keyed boundary remained in the DOM");
  }

  const beforeDuplicate = await list.locator(":scope > ax-each-item").allTextContents();
  await page.locator("#duplicate").click();
  const afterDuplicate = await list.locator(":scope > ax-each-item").allTextContents();
  if (JSON.stringify(afterDuplicate) !== JSON.stringify(beforeDuplicate)) {
    throw new Error("duplicate-key rejection partially changed the DOM");
  }

  const fallback = page.locator('ax-state-each[data-ax-each-render-status="fallback"]');
  const fallbackBefore = await fallback.textContent();
  await page.locator("#fallback").click();
  if ((await fallback.textContent()) !== fallbackBefore) throw new Error("fallback partially changed unsupported DOM");
  if ((await fallback.getAttribute("data-ax-each-status")) !== "refresh-required") {
    throw new Error("unsupported structure did not request a deterministic refresh");
  }

  const diagnostics = await page.evaluate(() => window.__axonyx.state.diagnostics());
  if (diagnostics.executor !== "wasm") throw new Error(`expected WASM executor, got ${diagnostics.executor}`);
  if (diagnostics.reconciledEachLists !== 4) {
    throw new Error(`expected four reconciliations, got ${diagnostics.reconciledEachLists}`);
  }
  if (diagnostics.rejectedEachLists !== 1) {
    throw new Error(`expected one rejected list, got ${diagnostics.rejectedEachLists}`);
  }
  if (diagnostics.eachRefreshesRequired !== 1) {
    throw new Error(`expected one refresh fallback, got ${diagnostics.eachRefreshesRequired}`);
  }
  if (localRequests.length !== 0) throw new Error(`local reconciliation made requests: ${localRequests.join(", ")}`);
  if (consoleErrors.length !== 0) throw new Error(`browser console errors: ${consoleErrors.join(" | ")}`);

  console.log("Reactive Each Chromium E2E passed.");
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
  rmSync(output, { recursive: true, force: true });
}

import { spawnSync } from "node:child_process";

const npmCli = process.env.npm_execpath;
const audit = npmCli
  ? spawnSync(process.execPath, [npmCli, "audit", "--json", "--omit=optional"], {
      encoding: "utf8",
    })
  : spawnSync("npm", ["audit", "--json", "--omit=optional"], {
      encoding: "utf8",
      shell: process.platform === "win32",
    });

if (audit.error) {
  console.error(`Unable to start npm audit: ${audit.error.message}`);
  process.exit(1);
}

let report;
try {
  report = JSON.parse(audit.stdout || "{}");
} catch {
  console.error(audit.stderr || audit.stdout || "npm audit returned no report");
  process.exit(1);
}

if (report.error || report.statusCode >= 400) {
  console.warn(`npm audit unavailable: ${report.message ?? "registry request failed"}`);
  process.exit(0);
}

const vulnerabilities = report.metadata?.vulnerabilities;
if (!vulnerabilities) {
  console.error(audit.stderr || "npm audit returned an incomplete report");
  process.exit(1);
}

const blocking = (vulnerabilities.high ?? 0) + (vulnerabilities.critical ?? 0);
if (blocking > 0) {
  console.error(
    `npm audit found ${vulnerabilities.high ?? 0} high and ${vulnerabilities.critical ?? 0} critical vulnerabilities`,
  );
  process.exit(1);
}

console.log("Browser test dependencies have no high or critical vulnerabilities.");

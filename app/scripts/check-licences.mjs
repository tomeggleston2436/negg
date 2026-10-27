// SPDX-License-Identifier: Apache-2.0
// Fails when any npm dependency (dev included) carries a licence outside the
// allow list in the repository's deny.toml. An `OR` expression
// passes when one operand is allowed; an `AND` expression needs every operand.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Reads the `allow = [...]` list of the repository's deny.toml (the single source). */
function readAllowList(denyPath) {
  const text = readFileSync(denyPath, "utf8");
  const section = text.split(/^\[licenses\]\s*$/m)[1];
  const match = section?.match(/^allow\s*=\s*\[([\s\S]*?)\]/m);
  if (!match) throw new Error(`check-licences: no [licenses] allow list in ${denyPath}`);
  const names = [...match[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  if (names.length === 0) throw new Error(`check-licences: empty allow list in ${denyPath}`);
  return new Set(names);
}

const APP_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ALLOWED = readAllowList(resolve(APP_DIR, "..", "deny.toml"));

/** True when an SPDX expression is satisfied by the allow list. */
export function licenceAllowed(expression) {
  const expr = expression.trim();
  if (ALLOWED.has(expr)) return true;
  const stripped = stripOuterParens(expr);
  if (stripped !== expr) return licenceAllowed(stripped);
  const orParts = splitTop(expr, "OR");
  if (orParts.length > 1) return orParts.some(licenceAllowed);
  const andParts = splitTop(expr, "AND");
  if (andParts.length > 1) return andParts.every(licenceAllowed);
  return false;
}

function stripOuterParens(expr) {
  if (!expr.startsWith("(") || !expr.endsWith(")")) return expr;
  let depth = 0;
  for (let i = 0; i < expr.length; i += 1) {
    if (expr[i] === "(") depth += 1;
    if (expr[i] === ")") depth -= 1;
    if (depth === 0 && i < expr.length - 1) return expr;
  }
  return expr.slice(1, -1).trim();
}

function splitTop(expr, op) {
  const parts = [];
  let depth = 0;
  let start = 0;
  const token = ` ${op} `;
  for (let i = 0; i < expr.length; i += 1) {
    if (expr[i] === "(") depth += 1;
    else if (expr[i] === ")") depth -= 1;
    else if (depth === 0 && expr.startsWith(token, i)) {
      parts.push(expr.slice(start, i));
      start = i + token.length;
      i += token.length - 1;
    }
  }
  parts.push(expr.slice(start));
  return parts.map((p) => p.trim()).filter((p) => p.length > 0);
}

function main() {
  const out = execFileSync("pnpm", ["licenses", "list", "--json"], {
    cwd: APP_DIR,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const byLicence = JSON.parse(out);
  const bad = [];
  let count = 0;
  for (const [key, packages] of Object.entries(byLicence)) {
    for (const pkg of packages) {
      count += 1;
      const licence = pkg.license ?? key;
      if (!licenceAllowed(String(licence))) {
        bad.push(`${pkg.name}@${pkg.versions.join(",")}: ${licence}`);
      }
    }
  }
  if (bad.length > 0) {
    console.error(`check-licences: ${bad.length} package(s) with a licence outside the allow list:`);
    for (const line of bad) console.error(`  ${line}`);
    process.exit(1);
  }
  console.log(`check-licences: ${count} packages, every licence in the deny.toml allow list (${ALLOWED.size} entries)`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}

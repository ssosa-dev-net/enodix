// Writes THIRD-PARTY-NOTICES.txt: every component of other people's that the
// release packages contain, its licence, and the licence texts those
// licences require to travel with the program (MIT, BSD, ISC, Apache-2.0 with
// any NOTICE file, MPL-2.0, Unicode, OFL, CDLA...).
//
//   node scripts/third-party-notices.mjs [--target <rust target>] [--out <file>]
//
// The Rust side is what `enodix` and `enodix-cli` link for the target: their
// normal dependencies, followed through Cargo's resolved graph (build and
// dev dependencies, and procedural macros, run only while building and ship
// nothing), plus the Rust standard library. The interface side is the npm
// packages that are not development-only in package-lock.json. A component's
// own licence files are used; one that ships none gets the standard text of
// its licence from the Rust toolchain's copies, under its authors' names.
// The release script runs this for each platform it builds
// (scripts/release.mjs); the file is generated, never committed.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const NOTICES_FILE = "THIRD-PARTY-NOTICES.txt";
/** The programs that ship; everything they link is listed. */
const SHIPPED = ["enodix", "enodix-cli"];
/** A licence file in a package's folder (not AUTHORS or a README). */
const LICENCE_FILE =
  /^(licen[cs]e|copying|copyright|notice|unlicense|patents)([-._ ].*)?$/i;

function cargo(args) {
  return execFileSync("cargo", args, {
    cwd: ROOT,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });
}

/** The target Rust builds for here (the release builds natively). */
function hostTarget() {
  return execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(
    /^host: (.+)$/m,
  )[1];
}

/** The licence files in `folder`, read, sorted by name. */
function licenceFiles(folder) {
  if (!existsSync(folder)) return [];
  const found = [];
  for (const name of readdirSync(folder).sort()) {
    const path = join(folder, name);
    const stat = statSync(path);
    if (stat.isDirectory() && /^licen[cs]es$/i.test(name)) {
      for (const inner of readdirSync(path).sort()) {
        const innerPath = join(path, inner);
        if (statSync(innerPath).isFile())
          found.push({
            name: `${name}/${inner}`,
            text: readFileSync(innerPath, "utf8"),
          });
      }
    } else if (stat.isFile() && LICENCE_FILE.test(name)) {
      found.push({ name, text: readFileSync(path, "utf8") });
    }
  }
  return found;
}

/** The standard licence texts the Rust toolchain ships, by SPDX id. */
function standardTexts() {
  const sysroot = execFileSync("rustc", ["--print", "sysroot"], {
    encoding: "utf8",
  }).trim();
  const folder = join(sysroot, "share", "doc", "rust", "licenses");
  const texts = new Map();
  if (existsSync(folder)) {
    for (const name of readdirSync(folder)) {
      if (name.endsWith(".txt"))
        texts.set(name.slice(0, -4), readFileSync(join(folder, name), "utf8"));
    }
  }
  return { sysroot, texts };
}

/** The SPDX ids an expression names ("MIT OR Apache-2.0" → both). */
function licenceIds(expression) {
  return (expression ?? "")
    .replace(/[()]/g, " ")
    .split(/\s+(?:OR|AND|WITH)\s+|\s*\/\s*/)
    .map((id) => id.trim())
    .filter(Boolean);
}

function rustComponents(target, standard) {
  const metadata = JSON.parse(
    cargo([
      "metadata",
      "--format-version",
      "1",
      "--locked",
      "--filter-platform",
      target,
    ]),
  );
  const packages = new Map(metadata.packages.map((p) => [p.id, p]));
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));
  const own = new Set(metadata.workspace_members);
  const roots = metadata.packages
    .filter((p) => own.has(p.id) && SHIPPED.includes(p.name))
    .map((p) => p.id);
  if (roots.length !== SHIPPED.length)
    throw new Error(`cargo metadata has not all of ${SHIPPED.join(", ")}`);

  const seen = new Set();
  const queue = [...roots];
  while (queue.length) {
    const id = queue.shift();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dep of nodes.get(id)?.deps ?? []) {
      const normal = dep.dep_kinds.some((k) => k.kind === null);
      const pkg = packages.get(dep.pkg);
      const procMacro = pkg.targets.every((t) => t.kind.includes("proc-macro"));
      if (normal && !procMacro) queue.push(dep.pkg);
    }
  }

  /** A package's own licence files (or its `license-file`). */
  const ownFiles = (pkg) => {
    const folder = dirname(pkg.manifest_path);
    const files = licenceFiles(folder);
    if (pkg.license_file && !files.length) {
      const path = join(folder, pkg.license_file);
      if (existsSync(path))
        return [{ name: pkg.license_file, text: readFileSync(path, "utf8") }];
    }
    return files;
  };

  /** The text of a licence that names no copyright holder (MPL-2.0), as
   * another crate under it ships it, where the toolchain has none. */
  const sharedText = (licence) => {
    const marker = { "MPL-2.0": "Mozilla Public License Version 2.0" }[licence];
    if (!marker) return undefined;
    for (const other of metadata.packages) {
      if (other.license !== licence) continue;
      const file = ownFiles(other).find((f) => f.text.includes(marker));
      if (file) return file.text;
    }
    return undefined;
  };

  const components = [];
  for (const id of seen) {
    if (own.has(id)) continue;
    const pkg = packages.get(id);
    let files = ownFiles(pkg);
    if (!files.length && pkg.repository) {
      // A crate published without its licence file: the file a sibling
      // from the same repository and under the same licence ships.
      const sibling = metadata.packages.find(
        (other) =>
          other.id !== pkg.id &&
          other.repository === pkg.repository &&
          other.license === pkg.license &&
          ownFiles(other).length,
      );
      if (sibling)
        files = ownFiles(sibling).map((file) => ({
          ...file,
          name: `${file.name} of ${sibling.name}, from the same repository`,
        }));
    }
    if (!files.length) {
      // No file of its own: the standard text of each licence it names.
      for (const licence of licenceIds(pkg.license)) {
        const text = standard.texts.get(licence) ?? sharedText(licence);
        if (text)
          files.push({
            name: `${licence} (standard text)`,
            text:
              `Copyright the authors of ${pkg.name}` +
              (pkg.authors.length ? `: ${pkg.authors.join(", ")}` : "") +
              `\n\n${text}`,
          });
      }
    }
    components.push({
      kind: "Rust crate",
      name: pkg.name,
      version: pkg.version,
      licence: pkg.license ?? "see its licence file",
      source: pkg.repository ?? `https://crates.io/crates/${pkg.name}`,
      files,
    });
  }

  // The standard library, linked into every Rust program.
  const stdFiles = ["MIT", "Apache-2.0"]
    .filter((id) => standard.texts.has(id))
    .map((id) => ({
      name: `${id} (standard text)`,
      text: `Copyright the Rust Project Developers\n\n${standard.texts.get(id)}`,
    }));
  components.push({
    kind: "Rust",
    name: "the Rust standard library",
    version: execFileSync("rustc", ["--version"], { encoding: "utf8" })
      .trim()
      .replace(/^rustc /, ""),
    licence: "MIT OR Apache-2.0",
    source: "https://github.com/rust-lang/rust",
    files: stdFiles,
  });
  return components;
}

function npmComponents(standard) {
  const lock = JSON.parse(
    readFileSync(join(ROOT, "package-lock.json"), "utf8"),
  );
  const components = [];
  for (const [path, entry] of Object.entries(lock.packages ?? {})) {
    if (!path.startsWith("node_modules/") || entry.dev || entry.devOptional)
      continue;
    const folder = join(ROOT, path);
    const manifest = join(folder, "package.json");
    const pkg = existsSync(manifest)
      ? JSON.parse(readFileSync(manifest, "utf8"))
      : {};
    const name = pkg.name ?? path.slice(path.lastIndexOf("node_modules/") + 13);
    const licence =
      entry.license ??
      pkg.license ??
      pkg.licenses?.map((l) => l.type).join(" OR ");
    let files = licenceFiles(folder);
    if (!files.length) {
      for (const id of licenceIds(licence)) {
        const text = standard.texts.get(id);
        if (text)
          files.push({
            name: `${id} (standard text)`,
            text: `Copyright the authors of ${name}\n\n${text}`,
          });
      }
    }
    const repository =
      typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url;
    components.push({
      kind: "npm package",
      name,
      version: entry.version ?? pkg.version,
      licence: licence ?? "see its licence file",
      source: repository ?? `https://www.npmjs.com/package/${name}`,
      files,
    });
  }
  return components;
}

/** The notices as text; `missing` names the components with no licence text. */
export function thirdPartyNotices({
  target = hostTarget(),
  product = "enodix",
} = {}) {
  const standard = standardTexts();
  const components = [
    ...rustComponents(target, standard),
    ...npmComponents(standard),
  ];
  components.sort(
    (a, b) => a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name),
  );

  // Identical texts (Apache-2.0 above all) are printed once.
  const texts = new Map();
  for (const component of components) {
    component.refs = component.files.map((file) => {
      const key = createHash("sha256").update(file.text.trim()).digest("hex");
      if (!texts.has(key))
        texts.set(key, {
          number: texts.size + 1,
          text: file.text.trim(),
          users: [],
        });
      const entry = texts.get(key);
      entry.users.push(`${component.name} ${component.version} (${file.name})`);
      return entry.number;
    });
  }

  const rule = "=".repeat(78);
  const lines = [
    `${product}: third-party notices`,
    "",
    `${product} is free software under the GNU General Public License,`,
    "version 3 (see LICENSE). It contains the components below, each made by",
    "others and used under its own licence. Their licence texts follow the",
    `list. Built for ${target}.`,
    "",
    rule,
    "Components",
    rule,
    "",
  ];
  for (const c of components) {
    lines.push(`${c.name} ${c.version} (${c.kind})`);
    lines.push(`  Licence: ${c.licence}`);
    lines.push(`  Source: ${c.source}`);
    lines.push(
      `  Licence text: ${c.refs.length ? c.refs.map((n) => `[${n}]`).join(", ") : "none found"}`,
    );
    lines.push("");
  }
  lines.push(rule, "Licence texts", rule, "");
  for (const { number, text, users } of texts.values()) {
    lines.push(
      `[${number}] Used by: ${users.join("; ")}`,
      "",
      text,
      "",
      "-".repeat(78),
      "",
    );
  }
  const missing = components
    .filter((c) => !c.files.length)
    .map((c) => `${c.name} ${c.version}`);
  return { text: `${lines.join("\n")}\n`, count: components.length, missing };
}

/** Writes the notices to `out` and fails if a component has no licence text. */
export function writeThirdPartyNotices(options = {}) {
  const out = options.out ?? join(ROOT, "target", "release", NOTICES_FILE);
  const { text, count, missing } = thirdPartyNotices(options);
  if (missing.length)
    throw new Error(`no licence text found for: ${missing.join(", ")}`);
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, text);
  return { out, count };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const arg = (name) => {
    const at = process.argv.indexOf(name);
    return at === -1 ? undefined : process.argv[at + 1];
  };
  try {
    const { out, count } = writeThirdPartyNotices({
      target: arg("--target"),
      out: arg("--out"),
    });
    console.log(`third-party notices: ${count} components in ${out}`);
  } catch (error) {
    console.error(`third-party notices: ${error.message}`);
    process.exit(1);
  }
}

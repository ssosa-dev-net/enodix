// Builds the Python bridge's wheel (`python/netterm`, pure Python) without
// Python: the app build runs on machines that may have none. The version is
// the app's (`Cargo.toml`'s `[workspace.package] version`), so the package
// always matches the headless binary it ships with; the app's name and the
// binary's name go into `netterm/_build.py`, which exists only in the wheel.
//
//   node scripts/build-wheel.mjs [output folder]   (default target/release/python)
//
// The wheel is reproducible: fixed timestamps, files in a fixed order.

import { createHash } from "node:crypto";
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateRawSync } from "node:zlib";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = join(ROOT, "python");
/** The distribution and its import package. */
export const DISTRIBUTION = "netterm";

/** The app's version, from the Cargo workspace. */
export function appVersion() {
  const cargo = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
  const section = cargo.split(/^\[workspace\.package\]\s*$/m)[1] ?? "";
  const version = /^version\s*=\s*"([^"]+)"/m.exec(section)?.[1];
  if (!version)
    throw new Error("Cargo.toml has no [workspace.package] version");
  return version;
}

/**
 * A semver version as Python writes it (PEP 440): `1.2.3`, `1.2.3-beta.1`
 * → `1.2.3b1`, `-alpha.N` → `aN`, `-rc.N` → `rcN`.
 */
export function pythonVersion(version) {
  const match =
    /^(\d+\.\d+\.\d+)(?:-(alpha|beta|rc)\.(\d+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(
      version,
    );
  if (!match) throw new Error(`cannot write version ${version} for Python`);
  const [, release, stage, number] = match;
  const short = { alpha: "a", beta: "b", rc: "rc" };
  return stage ? `${release}${short[stage]}${number}` : release;
}

/** The wheel's file name for the app version `version`. */
export function wheelName(version = appVersion()) {
  return `${DISTRIBUTION}-${pythonVersion(version)}-py3-none-any.whl`;
}

/** Where the app build leaves the wheel (`beforeBuildCommand`). */
export const RELEASE_FOLDER = join(ROOT, "target", "release", "python");

/** The headless binary's name (`crates/cli`'s `[[bin]]`), without `.exe`. */
export function cliName() {
  const manifest = readFileSync(
    join(ROOT, "crates", "cli", "Cargo.toml"),
    "utf8",
  );
  const bin = manifest.split(/^\[\[bin\]\]\s*$/m)[1] ?? "";
  const name = /^name\s*=\s*"([^"]+)"/m.exec(bin)?.[1];
  if (!name) throw new Error("crates/cli/Cargo.toml names no [[bin]]");
  return name;
}

function productName() {
  const config = JSON.parse(
    readFileSync(join(ROOT, "src-tauri", "tauri.conf.json"), "utf8"),
  );
  if (!config.productName)
    throw new Error("tauri.conf.json has no productName");
  return config.productName;
}

/** The package's own files, `/`-separated paths under `python/`, sorted. */
function packageFiles() {
  const folder = join(SOURCE, DISTRIBUTION);
  return readdirSync(folder, { withFileTypes: true })
    .filter(
      (entry) =>
        entry.isFile() &&
        (entry.name.endsWith(".py") || entry.name === "py.typed") &&
        entry.name !== "_build.py",
    )
    .map((entry) => `${DISTRIBUTION}/${entry.name}`)
    .sort();
}

function pyString(text) {
  return JSON.stringify(text); // a JSON string is a valid Python literal
}

function metadata(version) {
  const readme = readFileSync(join(SOURCE, "README.md"), "utf8");
  return [
    "Metadata-Version: 2.1",
    `Name: ${DISTRIBUTION}`,
    `Version: ${version}`,
    `Summary: The ${productName()} workspace as a Nornir inventory, with credentials from its profiles`,
    "License: GPL-3.0-only",
    "Requires-Python: >=3.10",
    "Provides-Extra: nornir",
    'Requires-Dist: nornir>=3.3; extra == "nornir"',
    "Classifier: Programming Language :: Python :: 3",
    "Classifier: Framework :: Nornir",
    "Description-Content-Type: text/markdown",
    "",
    readme,
  ].join("\n");
}

// A minimal zip writer: deflated entries, no extra fields, fixed times.
const DOS_DATE = (0 << 9) | (1 << 5) | 1; // 1980-01-01
const DOS_TIME = 0;

function zip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const { name, data } of entries) {
    const nameBytes = Buffer.from(name, "utf8");
    const packed = deflateRawSync(data, { level: 9 });
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4); // version needed
    local.writeUInt16LE(0x0800, 6); // UTF-8 names
    local.writeUInt16LE(8, 8); // deflate
    local.writeUInt16LE(DOS_TIME, 10);
    local.writeUInt16LE(DOS_DATE, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(packed.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    local.writeUInt16LE(0, 28);
    locals.push(local, nameBytes, packed);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(0x0314, 4); // made by: Unix, 2.0
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(8, 10);
    central.writeUInt16LE(DOS_TIME, 12);
    central.writeUInt16LE(DOS_DATE, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(packed.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE((0o100644 << 16) >>> 0, 38); // a regular file, rw-r--r--
    central.writeUInt32LE(offset, 42);
    centrals.push(central, nameBytes);
    offset += local.length + nameBytes.length + packed.length;
  }
  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

function recordLine(name, data) {
  const digest = createHash("sha256").update(data).digest("base64url");
  return `${name},sha256=${digest},${data.length}`;
}

/** Builds the wheel into `outDir` and returns its path. */
export function buildWheel(outDir = RELEASE_FOLDER) {
  const version = pythonVersion(appVersion());
  const distInfo = `${DISTRIBUTION}-${version}.dist-info`;
  const entries = packageFiles().map((name) => ({
    name,
    data: readFileSync(join(SOURCE, name)),
  }));
  entries.push({
    name: `${DISTRIBUTION}/_build.py`,
    data: Buffer.from(
      [
        '"""Written by scripts/build-wheel.mjs: what the wheel was built for."""',
        "",
        `VERSION = ${pyString(version)}`,
        `PRODUCT_NAME = ${pyString(productName())}`,
        `CLI_NAME = ${pyString(cliName())}`,
        "",
      ].join("\n"),
    ),
  });
  entries.sort((a, b) => (a.name < b.name ? -1 : 1));
  const meta = [
    { name: `${distInfo}/METADATA`, data: Buffer.from(metadata(version)) },
    {
      name: `${distInfo}/WHEEL`,
      data: Buffer.from(
        [
          "Wheel-Version: 1.0",
          "Generator: netterm build-wheel.mjs",
          "Root-Is-Purelib: true",
          "Tag: py3-none-any",
          "",
        ].join("\n"),
      ),
    },
    {
      name: `${distInfo}/entry_points.txt`,
      data: Buffer.from(
        [
          "[nornir.plugins.inventory]",
          `NettermInventory = ${DISTRIBUTION}.nornir:NettermInventory`,
          "",
        ].join("\n"),
      ),
    },
    {
      name: `${distInfo}/LICENSE`,
      data: readFileSync(join(ROOT, "LICENSE")),
    },
  ];
  const all = [...entries, ...meta];
  const record = [
    ...all.map(({ name, data }) => recordLine(name, data)),
    `${distInfo}/RECORD,,`,
    "",
  ].join("\n");
  all.push({ name: `${distInfo}/RECORD`, data: Buffer.from(record) });

  mkdirSync(outDir, { recursive: true });
  const file = join(outDir, wheelName());
  writeFileSync(file, zip(all));
  return file;
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  const out = process.argv[2] ? resolve(process.argv[2]) : undefined;
  console.log(`build-wheel: ${buildWheel(out)}`);
}

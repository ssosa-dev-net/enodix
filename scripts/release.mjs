// Builds a release in `target/dist/<version>/`. It publishes nothing:
// uploading the files is left to the person making the release.
//
//   npm run release
//   npm run release -- --unsigned
//
// `--unsigned` builds a pre-release with no updater: no release key, no
// signature, no `latest.json` and no update check's address, so those
// copies never look for or install an update. It needs none of the
// variables below and writes to `target/dist/<version>-unsigned/`.
//
// On Windows: the installer (NSIS, per user, signed for the updater), the
// portable zip and the update manifest `latest.json`. On Linux: the
// packages Tauri makes there (.deb, .rpm, .AppImage), whose copies only
// announce updates, so nothing is signed (`npm run release:linux` builds
// them in WSL from Windows: scripts/release-linux.mjs). Each platform
// replaces only its own files, so both sets can sit side by side, and
// `SHA256SUMS.txt` lists every file there.
//
// See docs/releasing.md. It reads (on Linux only the endpoint):
//
//   TAURI_SIGNING_PRIVATE_KEY           the updater's private key: the path
//                                       of the key file (never inside the
//                                       repository) or its contents
//   TAURI_SIGNING_PRIVATE_KEY_PASSWORD  its password
//   NETTERM_UPDATE_ENDPOINT             https address of `latest.json`, which
//                                       the app checks (built into the app)
//   NETTERM_RELEASE_URL                 https address of the folder this
//                                       release's files will be downloaded from
//   NETTERM_RELEASE_PAGE                optional: the release's web page
//                                       (default NETTERM_RELEASE_URL)
//   NETTERM_RELEASE_NOTES               optional: a short text for the notice
//   NETTERM_UPDATER_PUBKEY              optional: the public key (path or
//                                       contents); default
//                                       src-tauri/updater-key.pub

import { spawnSync } from "node:child_process";
import { createHash, createPublicKey, verify } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { wheelName } from "./build-wheel.mjs";
import { writeChecksums } from "./release-files.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PUBLIC_KEY_FILE = join(ROOT, "src-tauri", "updater-key.pub");
/** The file that makes a copy portable (`netterm_core::update::PORTABLE_MARKER`). */
const PORTABLE_MARKER = "portable.txt";
/** The updater's name for 64-bit Windows in `latest.json`. */
const PLATFORM = "windows-x86_64";
/** The headless binary, shipped beside the app (`crates/cli`). */
const CLI_EXE = "netterm-cli.exe";
/** The Linux build of it, installed in /usr/bin (on the PATH). */
const CLI_LINUX = "netterm-cli";
/** What each platform's build leaves in the release folder. */
const WINDOWS_FILES = /(-setup\.exe(\.sig)?|-portable\.zip|^latest\.json)$/;
const LINUX_FILES = /\.(deb|rpm|AppImage)$/;

function fail(message) {
  console.error(`release: ${message}`);
  process.exit(1);
}

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) fail(`${name} is not set (see docs/releasing.md)`);
  return value;
}

function httpsUrl(name, value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    fail(`${name} is not a web address: ${value}`);
  }
  if (url.protocol !== "https:") fail(`${name} must be an https address`);
  return url;
}

/** Whether `path` is inside the repository (where a key must never be). */
function insideRepository(path) {
  const rel = relative(ROOT, resolve(path));
  return !rel.startsWith("..") && !/^[a-zA-Z]:/.test(rel);
}

/** A value that is either a file's path or the contents themselves. */
function contentsOf(value) {
  return existsSync(value) && statSync(value).isFile()
    ? readFileSync(value, "utf8").trim()
    : value.trim();
}

function run(command, args, env = {}) {
  const result = spawnSync(command, args, {
    cwd: ROOT,
    stdio: "inherit",
    env: { ...process.env, ...env },
  });
  if (result.status !== 0) fail(`${command} ${args.join(" ")} failed`);
}

// --- Minisign, as the Tauri updater signs and checks ----------------------

/** The lines of a base64-wrapped minisign file (Tauri's .pub and .sig). */
function minisignLines(base64) {
  return Buffer.from(base64.trim(), "base64").toString("utf8").split(/\r?\n/);
}

function publicKey(base64) {
  const [, line] = minisignLines(base64);
  const raw = Buffer.from(line ?? "", "base64");
  if (raw.length !== 42 || raw.subarray(0, 2).toString() !== "Ed") {
    fail("the updater public key is not a minisign public key");
  }
  return {
    id: raw.subarray(2, 10),
    key: createPublicKey({
      key: {
        kty: "OKP",
        crv: "Ed25519",
        x: raw.subarray(10).toString("base64url"),
      },
      format: "jwk",
    }),
  };
}

/** Checks `file` against its signature the way the updater will, so that a
 * release signed with another key never goes out (it would strand every
 * installed copy). Returns the signature's trusted comment. */
function verifySignature(file, signature, pub) {
  const lines = minisignLines(signature);
  const sig = Buffer.from(lines[1] ?? "", "base64");
  const trusted = (lines[2] ?? "").replace(/^trusted comment: /, "");
  const global = Buffer.from(lines[3] ?? "", "base64");
  if (sig.length !== 74) fail(`${basename(file)}'s signature is not readable`);
  const algorithm = sig.subarray(0, 2).toString();
  if (!sig.subarray(2, 10).equals(pub.id)) {
    fail(
      `${basename(file)} was signed with another key than the public key given: ` +
        "check TAURI_SIGNING_PRIVATE_KEY and the public key",
    );
  }
  const data = readFileSync(file);
  const message =
    algorithm === "ED" ? createHash("blake2b512").update(data).digest() : data;
  if (!verify(null, message, pub.key, sig.subarray(10))) {
    fail(`${basename(file)}'s signature does not match the file`);
  }
  const signed = Buffer.concat([sig.subarray(10), Buffer.from(trusted)]);
  if (!verify(null, signed, pub.key, global)) {
    fail(`${basename(file)}'s signature comment does not match`);
  }
  return trusted;
}

// --- The release ----------------------------------------------------------

const tauriConfig = JSON.parse(
  readFileSync(join(ROOT, "src-tauri", "tauri.conf.json"), "utf8"),
);
const productName = tauriConfig.productName;
const version = readFileSync(join(ROOT, "Cargo.toml"), "utf8").match(
  /\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/,
)?.[1];
if (!version) fail("no version in Cargo.toml's [workspace.package]");
const npmVersion = JSON.parse(
  readFileSync(join(ROOT, "package.json"), "utf8"),
).version;
if (npmVersion !== version) {
  fail(
    `package.json has version ${npmVersion}, Cargo.toml ${version}: make them agree`,
  );
}
/** The Python bridge's wheel, of the same version. */
const wheel = wheelName(version);
/** The publisher, who is also the copyright holder: set once, as
 * `bundle.publisher` in tauri.conf.json (the headless binary's build.rs
 * makes the same copyright line for netterm-cli.exe). */
const publisher = tauriConfig.bundle?.publisher;
if (!publisher) fail("tauri.conf.json has no bundle.publisher");
const copyright = `Copyright (C) 2026 ${publisher}`;

/** A pre-release without the updater (see the top of this file). */
const unsigned = process.argv.includes("--unsigned");
const endpoint = unsigned
  ? null
  : httpsUrl("NETTERM_UPDATE_ENDPOINT", required("NETTERM_UPDATE_ENDPOINT"));
const dist = join(
  ROOT,
  "target",
  "dist",
  unsigned ? `${version}-unsigned` : version,
);
const tauriCli = join(ROOT, "node_modules", "@tauri-apps", "cli", "tauri.js");

/** Writes the build's Tauri configuration (merged over tauri.conf.json);
 * returns its path. */
function writeConfig(config) {
  const file = join(ROOT, "target", "release-config.json");
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify(config, null, 2));
  return file;
}

/** Removes this platform's files of an earlier build from `dist`, keeping
 * the other platform's. */
function clearOwnFiles(pattern) {
  mkdirSync(dist, { recursive: true });
  for (const name of readdirSync(dist)) {
    if (pattern.test(name) || name === "SHA256SUMS.txt")
      rmSync(join(dist, name), { force: true });
  }
}

/** The release key's public half and where this release will be served,
 * checked before anything is built. */
function signing() {
  const privateKey = required("TAURI_SIGNING_PRIVATE_KEY");
  if (existsSync(privateKey) && insideRepository(privateKey)) {
    fail(
      "the private key is inside the repository: move it out (see docs/releasing.md)",
    );
  }
  if (process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === undefined) {
    fail(
      "TAURI_SIGNING_PRIVATE_KEY_PASSWORD is not set (see docs/releasing.md)",
    );
  }
  const pubkeySource =
    process.env.NETTERM_UPDATER_PUBKEY?.trim() || PUBLIC_KEY_FILE;
  if (pubkeySource === PUBLIC_KEY_FILE && !existsSync(PUBLIC_KEY_FILE)) {
    fail(
      `no public key: create the release key and commit ${relative(ROOT, PUBLIC_KEY_FILE)}`,
    );
  }
  const pubkey = contentsOf(pubkeySource);
  const pub = publicKey(pubkey);
  const releaseUrl = httpsUrl(
    "NETTERM_RELEASE_URL",
    required("NETTERM_RELEASE_URL"),
  );
  const base = releaseUrl.href.endsWith("/")
    ? releaseUrl.href
    : `${releaseUrl.href}/`;
  const page = process.env.NETTERM_RELEASE_PAGE?.trim()
    ? httpsUrl("NETTERM_RELEASE_PAGE", process.env.NETTERM_RELEASE_PAGE.trim())
        .href
    : releaseUrl.href;
  return { pubkey, pub, base, page };
}

/** A text for an NSIS `!define`: `$` is NSIS's own, `"` would end it. */
function nsisText(text) {
  if (/["\r\n]/.test(text))
    fail(`cannot put ${JSON.stringify(text)} in the installer`);
  return text.replaceAll("$", "$$$$");
}

/** Writes the installer hooks Tauri is given: netterm's values, then
 * src-tauri/installer/hooks.nsh (which explains them); returns the path. */
function writeInstallerHooks() {
  const defines = {
    NETTERM_PRODUCT: productName,
    NETTERM_PUBLISHER: publisher,
    NETTERM_DESCRIPTION: tauriConfig.bundle?.shortDescription ?? productName,
    // Tauri's MANUFACTURER when no publisher is set: the vendor key of the
    // releases made before one was.
    NETTERM_OLD_VENDOR: tauriConfig.identifier.split(".")[1],
    NETTERM_CLI: CLI_EXE,
    NETTERM_WHEEL: wheel,
  };
  const file = join(ROOT, "target", "installer-hooks.nsh");
  const lines = Object.entries(defines).map(
    ([name, value]) => `!define ${name} "${nsisText(value)}"`,
  );
  lines.push(`!include "${join(ROOT, "src-tauri", "installer", "hooks.nsh")}"`);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, `${lines.join("\r\n")}\r\n`);
  return file;
}

function windows() {
  const signed = unsigned ? null : signing();
  // The updater exists only in a signed build: its key and the release
  // server.
  const configFile = writeConfig({
    bundle: {
      active: true,
      targets: ["nsis"],
      createUpdaterArtifacts: !unsigned,
      copyright,
      // The installer's licence page.
      licenseFile: join(ROOT, "LICENSE"),
      // The headless binary and the Python bridge's wheel (both built by
      // `beforeBuildCommand`) and the licence, installed beside the app.
      resources: {
        [`../target/release/${CLI_EXE}`]: CLI_EXE,
        [`../target/release/python/${wheel}`]: `python/${wheel}`,
        "../LICENSE": "LICENSE",
      },
      windows: {
        nsis: {
          installMode: "currentUser",
          // Old wheels, the headless binary in use, saved logins and the
          // old vendor key (src-tauri/installer/hooks.nsh).
          installerHooks: writeInstallerHooks(),
          // What "Delete the application data" deletes, and the hooks'
          // messages.
          languages: ["English"],
          customLanguageFiles: {
            English: join(ROOT, "src-tauri", "installer", "English.nsh"),
          },
        },
      },
    },
    ...(signed && {
      plugins: {
        updater: {
          pubkey: signed.pubkey,
          endpoints: [endpoint.href],
          // The signature names the version it was signed for, so a forged
          // manifest cannot pass an older signed release off as newer.
          requireSignedVersion: true,
          windows: { installMode: "passive" },
        },
      },
    }),
  });

  // Earlier output goes first, so that a failed build leaves nothing to upload.
  clearOwnFiles(WINDOWS_FILES);
  const bundleFolder = join(ROOT, "target", "release", "bundle", "nsis");
  rmSync(bundleFolder, { recursive: true, force: true });
  run(process.execPath, [tauriCli, "build", "--config", configFile]);

  const installerName = readdirSync(bundleFolder).find(
    (name) => name.endsWith("-setup.exe") && name.includes(`_${version}_`),
  );
  if (!installerName) fail(`no installer for ${version} in ${bundleFolder}`);
  const installer = join(bundleFolder, installerName);
  mkdirSync(dist, { recursive: true });
  copyFileSync(installer, join(dist, installerName));
  let signature = null;
  if (signed) {
    if (!existsSync(`${installer}.sig`))
      fail(`${installerName} has no updater signature`);
    signature = readFileSync(`${installer}.sig`, "utf8").trim();
    const trusted = verifySignature(installer, signature, signed.pub);
    // The app requires the signed version (requireSignedVersion): without it
    // every install of this release would fail.
    if (!trusted.split("\t").includes(`version:${version}`)) {
      fail(
        `${installerName}'s signature does not name version ${version}: ${trusted}`,
      );
    }
    console.log(
      `release: ${installerName} is signed by the release key (${trusted})`,
    );
    copyFileSync(`${installer}.sig`, join(dist, `${installerName}.sig`));
  }

  // The portable zip: the program, the headless binary, the Python bridge's
  // wheel, the marker that makes it portable (it only announces updates), the
  // README and the licence.
  const exeName = `${tauriConfig.mainBinaryName ?? "netterm"}.exe`;
  const portableName = `${productName}_${version}_x64-portable`;
  const stage = join(ROOT, "target", "dist", "stage", portableName);
  rmSync(dirname(stage), { recursive: true, force: true });
  mkdirSync(stage, { recursive: true });
  copyFileSync(join(ROOT, "target", "release", exeName), join(stage, exeName));
  copyFileSync(join(ROOT, "target", "release", CLI_EXE), join(stage, CLI_EXE));
  mkdirSync(join(stage, "python"));
  copyFileSync(
    join(ROOT, "target", "release", "python", wheel),
    join(stage, "python", wheel),
  );
  writeFileSync(
    join(stage, PORTABLE_MARKER),
    `This file makes this copy of ${productName} portable: it tells you when a new\r\n` +
      "version is out but never installs one. Download the new portable zip and\r\n" +
      "unpack it over this folder. Settings and known host keys are kept in your\r\n" +
      "user profile, not here.\r\n",
  );
  copyFileSync(join(ROOT, "README.md"), join(stage, "README.md"));
  copyFileSync(join(ROOT, "LICENSE"), join(stage, "LICENSE"));
  const zip = join(dist, `${portableName}.zip`);
  // Windows' own bsdtar (not Git's GNU tar, which cannot write zip files):
  // unlike PowerShell's Compress-Archive it stores `/` separators, which every
  // unzip tool reads.
  const windowsTar = join(
    process.env.SystemRoot ?? "C:\\Windows",
    "System32",
    "tar.exe",
  );
  run(windowsTar, ["-a", "-c", "-f", zip, "-C", dirname(stage), portableName]);
  rmSync(dirname(stage), { recursive: true, force: true });

  if (!signed) return;
  // The manifest the app checks: the Tauri updater's static format, plus the
  // page that the notice links to.
  const { base, page } = signed;
  const manifest = {
    version,
    notes:
      process.env.NETTERM_RELEASE_NOTES?.trim() || `${productName} ${version}`,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    page,
    platforms: {
      [PLATFORM]: {
        signature,
        url: new URL(encodeURIComponent(installerName), base).href,
      },
    },
  };
  writeFileSync(
    join(dist, "latest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
}

function linux() {
  clearOwnFiles(LINUX_FILES);
  const bundles = join(ROOT, "target", "release", "bundle");
  const formats = { deb: "deb", rpm: "rpm", appimage: "AppImage" };
  for (const folder of Object.keys(formats))
    rmSync(join(bundles, folder), { recursive: true, force: true });
  // The headless binary goes to /usr/bin, beside the app's own program
  // (where the app looks for it) and on the PATH (where the Python bridge
  // finds it); the wheel to /usr/lib/<product>/python, the resource folder.
  const cli = { [`/usr/bin/${CLI_LINUX}`]: `../target/release/${CLI_LINUX}` };
  const configFile = writeConfig({
    bundle: {
      active: true,
      targets: Object.keys(formats),
      resources: {
        [`../target/release/python/${wheel}`]: `python/${wheel}`,
      },
      linux: {
        deb: { files: cli },
        rpm: { files: cli },
        appimage: { files: cli },
      },
    },
    // Only the update check's address: off Windows the app never installs
    // an update (InstallKind::detect), so there is no updater plugin, no
    // key and no signature. An unsigned pre-release checks nothing at all.
    ...(endpoint && { plugins: { updater: { endpoints: [endpoint.href] } } }),
  });
  run(process.execPath, [tauriCli, "build", "--config", configFile]);
  mkdirSync(dist, { recursive: true });
  for (const [folder, extension] of Object.entries(formats)) {
    const made = existsSync(join(bundles, folder))
      ? readdirSync(join(bundles, folder)).filter((name) =>
          name.endsWith(`.${extension}`),
        )
      : [];
    if (made.length !== 1)
      fail(
        `expected one .${extension} in ${join(bundles, folder)}, found ${made.length}`,
      );
    copyFileSync(join(bundles, folder, made[0]), join(dist, made[0]));
  }
}

if (process.platform === "win32") windows();
else if (process.platform === "linux") linux();
else fail(`releases are not built on ${process.platform}`);

const files = writeChecksums(dist);
console.log(
  `\nrelease: ${productName} ${version} is ready in ${relative(ROOT, dist)}:`,
);
for (const name of files) console.log(`  ${name}`);
console.log(
  endpoint
    ? "\nNothing was uploaded. Upload these files to the release, and the" +
        ` Windows build's latest.json to ${endpoint.href} (see docs/releasing.md).`
    : "\nNothing was uploaded. This is an unsigned pre-release: its copies" +
        " never check for or install an update (see docs/releasing.md).",
);

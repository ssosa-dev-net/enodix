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
// See docs/dev/releasing.md. It reads (on Linux only the endpoint):
//
//   TAURI_SIGNING_PRIVATE_KEY           the updater's private key: the path
//                                       of the key file (never inside the
//                                       repository) or its contents
//   TAURI_SIGNING_PRIVATE_KEY_PASSWORD  its password
//   ENODIX_UPDATE_ENDPOINT             https address of `latest.json`, which
//                                       the app checks (built into the app)
//   ENODIX_RELEASE_URL                 https address of the folder this
//                                       release's files will be downloaded from
//   ENODIX_RELEASE_PAGE                optional: the release's web page
//                                       (default ENODIX_RELEASE_URL)
//   ENODIX_RELEASE_NOTES               optional: a short text for the notice
//   ENODIX_UPDATER_PUBKEY              optional: the public key (path or
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
import {
  NOTICES_FILE,
  writeThirdPartyNotices,
} from "./third-party-notices.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PUBLIC_KEY_FILE = join(ROOT, "src-tauri", "updater-key.pub");
/** The file that makes a copy portable (`enodix_core::update::PORTABLE_MARKER`). */
const PORTABLE_MARKER = "portable.txt";
/** Marks the Linux packages' copies (`enodix_core::update::PACKAGE_MARKER`). */
const PACKAGE_MARKER = "package.txt";
/** The updater's name for 64-bit Windows in `latest.json`. */
const PLATFORM = "windows-x86_64";
/** The headless binary, shipped beside the app (`crates/cli`). */
const CLI_EXE = "enodix-cli.exe";
/** The Linux build of it, installed in /usr/bin (on the PATH). */
const CLI_LINUX = "enodix-cli";
/** What each platform's build leaves in the release folder. */
const WINDOWS_FILES = /(-setup\.exe(\.sig)?|-portable\.zip|^latest\.json)$/;
const LINUX_FILES = /\.(deb|rpm|AppImage)$/;

function fail(message) {
  console.error(`release: ${message}`);
  process.exit(1);
}

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) fail(`${name} is not set (see docs/dev/releasing.md)`);
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
/** Where each Linux package carries the licence (the GPL): Debian's
 * copyright file, the licence folder of RPM-based systems, and the same in
 * the AppImage (relative to its own root). The packages are named after the
 * product. */
const LINUX_LICENCE = {
  deb: `/usr/share/doc/${productName}/copyright`,
  rpm: `/usr/share/licenses/${productName}/LICENSE`,
  appimage: `/usr/share/licenses/${productName}/LICENSE`,
};
/** Where each Linux package carries the third-party notices: beside the
 * licence (scripts/third-party-notices.mjs writes them). */
const LINUX_NOTICES = {
  deb: `/usr/share/doc/${productName}/${NOTICES_FILE}`,
  rpm: `/usr/share/licenses/${productName}/${NOTICES_FILE}`,
  appimage: `/usr/share/licenses/${productName}/${NOTICES_FILE}`,
};
/** The notices as built for this platform (generated, never committed). */
const NOTICES = join(ROOT, "target", "release", NOTICES_FILE);
/** The AppRun hook (src-tauri/appimage/desktop-style.sh) that makes the
 * AppImage start dark on a desktop whose dark look is a style (#143). */
const APPIMAGE_STYLE_HOOK = "/apprun-hooks/enodix-desktop-style.sh";
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
 * makes the same copyright line for enodix-cli.exe). */
const publisher = tauriConfig.bundle?.publisher;
if (!publisher) fail("tauri.conf.json has no bundle.publisher");
const copyright = `Copyright (C) 2026 ${publisher}`;

/** A pre-release without the updater (see the top of this file). */
const unsigned = process.argv.includes("--unsigned");
const endpoint = unsigned
  ? null
  : httpsUrl("ENODIX_UPDATE_ENDPOINT", required("ENODIX_UPDATE_ENDPOINT"));
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
      "the private key is inside the repository: move it out (see docs/dev/releasing.md)",
    );
  }
  if (process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === undefined) {
    fail(
      "TAURI_SIGNING_PRIVATE_KEY_PASSWORD is not set (see docs/dev/releasing.md)",
    );
  }
  // Kept here and taken out of the environment: nothing this script starts
  // inherits the key (the build runs every crate's build script and npm's
  // tools, any of which could read it), and only the signer started after
  // the build is given it (`sign`).
  const key = {
    privateKey: contentsOf(privateKey),
    password: process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
  };
  delete process.env.TAURI_SIGNING_PRIVATE_KEY;
  delete process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
  const pubkeySource =
    process.env.ENODIX_UPDATER_PUBKEY?.trim() || PUBLIC_KEY_FILE;
  if (pubkeySource === PUBLIC_KEY_FILE && !existsSync(PUBLIC_KEY_FILE)) {
    fail(
      `no public key: create the release key and commit ${relative(ROOT, PUBLIC_KEY_FILE)}`,
    );
  }
  const pubkey = contentsOf(pubkeySource);
  const pub = publicKey(pubkey);
  const releaseUrl = httpsUrl(
    "ENODIX_RELEASE_URL",
    required("ENODIX_RELEASE_URL"),
  );
  const base = releaseUrl.href.endsWith("/")
    ? releaseUrl.href
    : `${releaseUrl.href}/`;
  const page = process.env.ENODIX_RELEASE_PAGE?.trim()
    ? httpsUrl("ENODIX_RELEASE_PAGE", process.env.ENODIX_RELEASE_PAGE.trim())
        .href
    : releaseUrl.href;
  return { pubkey, pub, base, page, key };
}

/** Signs `file` for the updater with the release key, naming `version` in
 * the signature (the app requires it): `<file>.sig`. The signer is the one
 * program given the key, after the build is done. */
function sign(file, key) {
  run(
    process.execPath,
    [tauriCli, "signer", "sign", "--app-version", version, file],
    {
      TAURI_SIGNING_PRIVATE_KEY: key.privateKey,
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: key.password,
    },
  );
}

/** A text for an NSIS `!define`: `$` is NSIS's own, `"` would end it. */
function nsisText(text) {
  if (/["\r\n]/.test(text))
    fail(`cannot put ${JSON.stringify(text)} in the installer`);
  return text.replaceAll("$", "$$$$");
}

/** Writes the installer hooks Tauri is given: enodix's values, then
 * src-tauri/installer/hooks.nsh (which explains them); returns the path. */
function writeInstallerHooks() {
  const defines = {
    ENODIX_PRODUCT: productName,
    ENODIX_PUBLISHER: publisher,
    ENODIX_DESCRIPTION: tauriConfig.bundle?.shortDescription ?? productName,
    // Tauri's MANUFACTURER when no publisher is set: the vendor key of the
    // releases made before one was.
    ENODIX_OLD_VENDOR: tauriConfig.identifier.split(".")[1],
    ENODIX_CLI: CLI_EXE,
    ENODIX_WHEEL: wheel,
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

/** Writes this platform's third-party notices, or fails the release. */
function notices() {
  try {
    const { count } = writeThirdPartyNotices({
      out: NOTICES,
      product: productName,
    });
    console.log(`release: ${NOTICES_FILE} lists ${count} components`);
  } catch (error) {
    fail(`third-party notices: ${error.message}`);
  }
}

function windows() {
  const signed = unsigned ? null : signing();
  notices();
  // The updater exists only in a signed build: its key and the release
  // server.
  const configFile = writeConfig({
    bundle: {
      active: true,
      targets: ["nsis"],
      // Signed after the build, by `sign`: the build never sees the key.
      createUpdaterArtifacts: false,
      copyright,
      // The installer's licence page.
      licenseFile: join(ROOT, "LICENSE"),
      // The headless binary and the Python bridge's wheel (both built by
      // `beforeBuildCommand`), the licence and the third-party notices,
      // installed beside the app.
      resources: {
        [`../target/release/${CLI_EXE}`]: CLI_EXE,
        [`../target/release/python/${wheel}`]: `python/${wheel}`,
        "../LICENSE": "LICENSE",
        [`../target/release/${NOTICES_FILE}`]: NOTICES_FILE,
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
    rmSync(`${installer}.sig`, { force: true });
    sign(installer, signed.key);
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
  // README, the licence and the third-party notices.
  const exeName = `${tauriConfig.mainBinaryName ?? "enodix"}.exe`;
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
  copyFileSync(NOTICES, join(stage, NOTICES_FILE));
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
      process.env.ENODIX_RELEASE_NOTES?.trim() || `${productName} ${version}`,
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
  notices();
  const bundles = join(ROOT, "target", "release", "bundle");
  const formats = { deb: "deb", rpm: "rpm", appimage: "AppImage" };
  for (const folder of Object.keys(formats))
    rmSync(join(bundles, folder), { recursive: true, force: true });
  // The headless binary goes to /usr/bin, beside the app's own program
  // (where the app looks for it) and on the PATH (where the Python bridge
  // finds it); the wheel to /usr/lib/<product>/python, the resource folder;
  // the GPL where each kind of package keeps its licence (LINUX_LICENCE),
  // and the third-party notices beside it (LINUX_NOTICES).
  const cli = { [`/usr/bin/${CLI_LINUX}`]: `../target/release/${CLI_LINUX}` };
  const withLicence = (format) => ({
    ...cli,
    [LINUX_LICENCE[format]]: "../LICENSE",
    [LINUX_NOTICES[format]]: `../target/release/${NOTICES_FILE}`,
  });
  // The .deb and the .rpm (not the AppImage) carry PACKAGE_MARKER in the
  // resource folder: the app then knows the package manager updates it
  // (InstallKind::detect).
  const marker = join(ROOT, "target", "release", PACKAGE_MARKER);
  // A fresh checkout has no target/release yet: the build makes it later.
  mkdirSync(dirname(marker), { recursive: true });
  writeFileSync(
    marker,
    `This copy of ${productName} was installed by a system package: ` +
      "update it with the package manager.\n",
  );
  const packaged = (format) => ({
    ...withLicence(format),
    [`/usr/lib/${productName}/${PACKAGE_MARKER}`]: `../target/release/${PACKAGE_MARKER}`,
  });
  // The .rpm records its files but not every folder they are in, so
  // `rpm -e` left /usr/lib/<product>/python and /usr/share/licenses/<product>
  // behind, empty. Its post-removal script removes those folders once the
  // package is gone ($1 is 0; an upgrade passes 1), and only if empty.
  // (dpkg removes the .deb's own folders.)
  const rpmPostRemove = join(ROOT, "target", "release", "rpm-post-remove.sh");
  const emptyFolders = [
    `/usr/lib/${productName}/python`,
    `/usr/lib/${productName}`,
    `/usr/share/licenses/${productName}`,
  ];
  writeFileSync(
    rpmPostRemove,
    `# ${productName}: the folders its files were in, once it is removed.\n` +
      'if [ "$1" = 0 ]; then\n' +
      emptyFolders
        .map((folder) => `  rmdir "${folder}" 2>/dev/null || :\n`)
        .join("") +
      "fi\n",
  );
  const configFile = writeConfig({
    bundle: {
      active: true,
      targets: Object.keys(formats),
      resources: {
        [`../target/release/python/${wheel}`]: `python/${wheel}`,
      },
      linux: {
        deb: { files: packaged("deb") },
        rpm: { files: packaged("rpm"), postRemoveScript: rpmPostRemove },
        appimage: { files: withLicence("appimage") },
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
    if (folder === "appimage") addStyleHook(join(bundles, folder, made[0]));
    checkLinuxPackage(folder, join(bundles, folder, made[0]));
    copyFileSync(join(bundles, folder, made[0]), join(dist, made[0]));
  }
}

/** Adds enodix's AppRun hook (src-tauri/appimage/desktop-style.sh, #143)
 * to the AppImage Tauri made, after linuxdeploy's GTK hook. Tauri's
 * `appimage.files` reach only the AppDir's `usr/`, so the AppImage is
 * unpacked, the hook put in `apprun-hooks/` and sourced by AppRun, and
 * packed again: the same runtime in front of a squashfs made with the same
 * compression and block size. */
function addStyleHook(file) {
  const output = (command, args) => {
    const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
    if (result.error?.code === "ENOENT")
      fail(
        `${command} is needed to add the AppImage's hook (docs/dev/linux.md)`,
      );
    if (result.status !== 0)
      fail(`${command} ${args.join(" ")} failed: ${result.stderr}`);
    return result.stdout;
  };
  const offset = Number(output(file, ["--appimage-offset"]).trim());
  if (!(offset > 0)) fail(`${relative(ROOT, file)} gives no squashfs offset`);
  const about = output("unsquashfs", ["-s", "-o", String(offset), file]);
  const compression = about.match(/^Compression (\S+)/m)?.[1];
  const blockSize = about.match(/^Block size (\d+)/m)?.[1];
  if (!compression || !blockSize)
    fail(`cannot read the squashfs of ${relative(ROOT, file)}: ${about}`);
  const work = join(dirname(file), "style-hook");
  rmSync(work, { recursive: true, force: true });
  mkdirSync(work);
  const tree = join(work, "root");
  output("unsquashfs", ["-q", "-n", "-o", String(offset), "-d", tree, file]);
  const gtkHook = 'source "$this_dir"/apprun-hooks/"linuxdeploy-plugin-gtk.sh"';
  const appRun = readFileSync(join(tree, "AppRun"), "utf8");
  if (!appRun.split("\n").includes(gtkHook))
    fail(`the AppImage's AppRun does not source the GTK hook: ${appRun}`);
  const hookName = basename(APPIMAGE_STYLE_HOOK);
  copyFileSync(
    join(ROOT, "src-tauri", "appimage", "desktop-style.sh"),
    join(tree, APPIMAGE_STYLE_HOOK),
  );
  writeFileSync(
    join(tree, "AppRun"),
    appRun.replace(
      gtkHook,
      () => `${gtkHook}\nsource "$this_dir"/apprun-hooks/"${hookName}"`,
    ),
  );
  const squashfs = join(work, "root.squashfs");
  output("mksquashfs", [
    tree,
    squashfs,
    "-all-root",
    "-noappend",
    "-no-progress",
    "-comp",
    compression,
    "-b",
    blockSize,
  ]);
  const runtime = readFileSync(file).subarray(0, offset);
  writeFileSync(file, Buffer.concat([runtime, readFileSync(squashfs)]), {
    mode: 0o755,
  });
  rmSync(work, { recursive: true, force: true });
  console.log(`release: ${relative(ROOT, file)} sources ${hookName}`);
}

/** The files a Linux package lists (paths without a leading `./`), read
 * with the distribution's own tools. */
function linuxPackageFiles(format, file) {
  const output = (command, args) => {
    const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
    if (result.error?.code === "ENOENT")
      fail(
        `${command} is needed to check the packages' contents: install it (docs/dev/linux.md)`,
      );
    if (result.status !== 0)
      fail(`${command} ${args.join(" ")} failed: ${result.stderr}`);
    return result.stdout;
  };
  let listed;
  if (format === "deb") {
    // Lines like `-rw-r--r-- 0/0 1234 2026-11-19 20:27 ./usr/bin/enodix`.
    listed = output("dpkg-deb", ["--contents", file])
      .split("\n")
      .map((line) => line.split(/\s+/).slice(5).join(" "));
  } else if (format === "rpm") {
    listed = output("sh", [
      "-c",
      'rpm2cpio "$1" | cpio --list --quiet',
      "sh",
      file,
    ]).split("\n");
  } else {
    const offset = output(file, ["--appimage-offset"]).trim();
    listed = output("unsquashfs", ["-l", "-o", offset, file])
      .split("\n")
      .map((line) => line.replace(/^squashfs-root/, ""));
  }
  return new Set(
    listed
      .map((path) => path.trim().replace(/^\.?\/?/, "/"))
      .filter((path) => path !== "/"),
  );
}

/** Fails unless the package holds the headless binary, the wheel and the
 * licence where they are meant to be. */
function checkLinuxPackage(format, file) {
  const files = linuxPackageFiles(format, file);
  const expected = [
    `/usr/bin/${CLI_LINUX}`,
    `/usr/lib/${productName}/python/${wheel}`,
    LINUX_LICENCE[format],
    LINUX_NOTICES[format],
  ];
  // The .deb and the .rpm say a package manager updates them; the AppImage
  // must not (InstallKind::detect).
  const marker = `/usr/lib/${productName}/${PACKAGE_MARKER}`;
  if (format !== "appimage") expected.push(marker);
  else expected.push(APPIMAGE_STYLE_HOOK);
  if (format === "appimage" && files.has(marker))
    fail(`${relative(ROOT, file)} holds ${marker}, which marks a package`);
  const missing = expected.filter((path) => !files.has(path));
  if (missing.length)
    fail(`${relative(ROOT, file)} lacks ${missing.join(", ")}`);
  console.log(`release: ${relative(ROOT, file)} holds ${expected.join(", ")}`);
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
        ` Windows build's latest.json to ${endpoint.href} (see docs/dev/releasing.md).`
    : "\nNothing was uploaded. This is an unsigned pre-release: its copies" +
        " never check for or install an update (see docs/dev/releasing.md).",
);

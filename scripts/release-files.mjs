// The release folder's checksum file, shared by the release script and the
// Linux release run from Windows (both add files to the same folder).

import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const CHECKSUMS = "SHA256SUMS.txt";

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

/** Writes `SHA256SUMS.txt` (`sha256sum` format) for every other file in
 * `folder`; returns the names of every file there, itself last. */
export function writeChecksums(folder) {
  const files = readdirSync(folder)
    .filter((name) => name !== CHECKSUMS)
    .sort();
  writeFileSync(
    join(folder, CHECKSUMS),
    files.map((name) => `${sha256(join(folder, name))}  ${name}\n`).join(""),
  );
  return [...files, CHECKSUMS];
}

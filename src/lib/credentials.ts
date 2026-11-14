// The interface's view of the core's credential profiles. Secrets go to the
// core when a profile is saved and never come back: the interface only
// sees what a profile is, not what it holds.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

/** A stored profile, without its secrets. */
export interface ProfileSummary {
  name: string;
  username: string;
  auth:
    | { kind: "password" }
    | { kind: "private_key"; key_file: string; has_passphrase: boolean }
    | { kind: "agent" };
  has_enable_secret: boolean;
}

export interface ProfilesView {
  /** The stored profiles, by name. */
  profiles: ProfileSummary[];
  /** Profile names the workspace uses that are not set up here. */
  missing: string[];
}

/**
 * A profile as entered. A secret left null keeps the stored one; an empty
 * passphrase or enable secret removes it.
 */
export interface ProfileDraft {
  name: string;
  username: string;
  auth:
    | { kind: "password"; password: string | null }
    | { kind: "private_key"; key_file: string; passphrase: string | null }
    | { kind: "agent" };
  enable_secret: string | null;
}

/** How a host logs in. */
export type HostLogin =
  | { kind: "profile"; profile: string; username: string }
  | { kind: "notSetUp"; profile: string }
  | { kind: "prompt" };

export function listProfiles() {
  return invoke<ProfilesView>("profiles_list");
}

/** Creates a profile, or changes the one called `previous`. */
export function saveProfile(previous: string | null, draft: ProfileDraft) {
  return invoke<ProfilesView>("profile_save", { previous, draft });
}

export function deleteProfile(name: string) {
  return invoke<ProfilesView>("profile_delete", { name });
}

export function hostLogin(id: string) {
  return invoke<HostLogin>("host_login", { id });
}

/** Asks the user for a private key file; null if they cancel. */
export async function pickKeyFile(): Promise<string | null> {
  const file = await open({ title: "Choose a private key file" });
  return typeof file === "string" ? file : null;
}

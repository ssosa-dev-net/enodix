// The interface's view of the core's credential profiles. Secrets go to the
// core when a profile is saved and never come back: the interface only
// sees what a profile is, not what it holds.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { Protocol } from "./workspace";

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
  /** The names of the workspace's named secrets; values never come back. */
  secrets: string[];
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
  | { kind: "prompt" }
  /** Over telnet: connect at once; the device asks in the terminal. */
  | { kind: "telnet" }
  /** On its serial console: connect at once, as for telnet. */
  | { kind: "serial" };

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

/**
 * Adds a named secret, or (`replace`) gives the one called `name` a new
 * value. The value goes to the core and is never sent back.
 */
export function saveSecret(name: string, value: string, replace: boolean) {
  return invoke<ProfilesView>("secret_save", { name, value, replace });
}

export function deleteSecret(name: string) {
  return invoke<ProfilesView>("secret_delete", { name });
}

/**
 * How the host logs in over `protocol`, or over its own protocol setting
 * if none is given.
 */
export function hostLogin(id: string, protocol: Protocol | null = null) {
  return invoke<HostLogin>("host_login", { id, protocol });
}

/** Asks the user for a private key file; null if they cancel. */
export async function pickKeyFile(): Promise<string | null> {
  const file = await open({ title: "Choose a private key file" });
  return typeof file === "string" ? file : null;
}

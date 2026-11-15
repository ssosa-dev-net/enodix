// The open workspace's credential profiles and the names of its secrets,
// as the interface shows them, and the vault's state, for the Credentials
// side view. Fetched again whenever the workspace changes, since the
// profiles it names may change with it, and after every vault change.

import * as api from "./credentials";
import type { ProfileDraft, ProfilesView } from "./credentials";
import { vaultStatus, type VaultStatus } from "./vault";

export const profiles = $state({
  view: {
    profiles: [],
    missing: [],
    secrets: [],
    needs_vault: false,
  } as ProfilesView,
  /** Why the profiles could not be read, if they could not. */
  error: null as string | null,
  /** Whether the vault is off, locked or unlocked; null until known. */
  vault: null as VaultStatus | null,
});

// Answers can arrive out of order; only the latest request's counts.
let latest = 0;

/** Shows the profiles `request` returns; resolves to its error, if any. */
async function adopt(request: Promise<ProfilesView>): Promise<string | null> {
  const ticket = ++latest;
  try {
    const view = await request;
    if (ticket === latest) {
      profiles.view = view;
      profiles.error = null;
    }
    return null;
  } catch (error) {
    return String(error);
  }
}

export async function refreshProfiles() {
  const [error] = await Promise.all([
    adopt(api.listProfiles()),
    refreshVault(),
  ]);
  profiles.error = error;
}

/** Asks the core whether the vault is off, locked or unlocked. */
async function refreshVault() {
  try {
    profiles.vault = await vaultStatus();
  } catch {
    profiles.vault = null;
  }
}

/** Resolves to an error to show, or null once saved. */
export function saveProfile(previous: string | null, draft: ProfileDraft) {
  return adopt(api.saveProfile(previous, draft));
}

/** Resolves to an error to show, or null once deleted. */
export function deleteProfile(name: string) {
  return adopt(api.deleteProfile(name));
}

/** Resolves to an error to show, or null once saved. */
export function saveSecret(name: string, value: string, replace: boolean) {
  return adopt(api.saveSecret(name, value, replace));
}

/** Resolves to an error to show, or null once deleted. */
export function deleteSecret(name: string) {
  return adopt(api.deleteSecret(name));
}

/** Every profile name worth offering: the stored ones and the missing. */
export function profileNames(): string[] {
  return [
    ...profiles.view.profiles.map((profile) => profile.name),
    ...profiles.view.missing,
  ];
}

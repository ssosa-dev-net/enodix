// The open workspace's credential profiles and the names of its secrets,
// as the interface shows them, and whether the profiles dialog is open.
// Fetched again whenever the workspace changes, since the profiles it
// names may change with it.

import * as api from "./credentials";
import type { ProfileDraft, ProfilesView } from "./credentials";

export const profiles = $state({
  view: { profiles: [], missing: [], secrets: [] } as ProfilesView,
  /** Why the profiles could not be read, if they could not. */
  error: null as string | null,
  /** Whether the profiles dialog is open. */
  dialog: false,
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
  profiles.error = await adopt(api.listProfiles());
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

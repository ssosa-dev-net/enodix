// The vault: the app's own encrypted credential store behind a master
// password. The password goes to the core for one call and is never kept
// here.

import { invoke } from "@tauri-apps/api/core";

/** No vault file (the keychain is used), or one that is locked or open. */
export type VaultStatus = "off" | "locked" | "unlocked";

/** Why unlocking or turning on did not work. */
export interface VaultFailure {
  kind: "wrongPassword" | "damaged" | "invalid" | "failed";
  message: string;
}

export function vaultStatus() {
  return invoke<VaultStatus>("vault_status");
}

export function unlockVault(password: string) {
  return invoke<void>("vault_unlock", { password });
}

/** Moves every workspace's profiles and secrets into a new vault. */
export function turnOnVault(password: string, repeated: string) {
  return invoke<VaultStatus>("vault_turn_on", { password, repeated });
}

export function quitApp() {
  return invoke<void>("app_quit");
}

/** The failure an invoke rejected with, whatever shape it came in. */
export function vaultFailure(error: unknown): VaultFailure {
  if (typeof error === "object" && error !== null && "kind" in error)
    return error as VaultFailure;
  return { kind: "failed", message: String(error) };
}

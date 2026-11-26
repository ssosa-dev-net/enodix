// The ways into a host besides its own protocol, as the details sheet and
// the tree's context menu offer them: which protocols, and why one cannot
// be used (the core refuses those anyway; this only says so beforehand).

import type { Transport } from "./session";
import type { EffectiveSettings, Protocol } from "./workspace";

export const PROTOCOL_NAMES: Record<Protocol, string> = {
  ssh: "SSH",
  telnet: "Telnet",
  serial: "Serial",
};

/** What the header calls each kind of session. */
export const TRANSPORT_NAMES: Record<Transport, string> = {
  ...PROTOCOL_NAMES,
  local: "Local",
};

export interface WayIn {
  protocol: Protocol;
  /** Why it cannot be used, or null. */
  blocked: string | null;
}

const NO_PORT = "Choose the serial port in the serial line settings first";

/**
 * The jump host `effective` reaches the host `id` through (SSH and telnet
 * both go through it). A host that inherits itself as its jump host
 * connects directly.
 */
export function jumpHostOf(
  id: string,
  effective: EffectiveSettings | null,
): string | null {
  const jump = effective?.jump_host?.value;
  return jump && jump.toLowerCase() !== id.toLowerCase() ? jump : null;
}

/** Why the host's own way in cannot be used (serial without a port). */
export function ownWayBlocked(
  effective: EffectiveSettings | null,
): string | null {
  return effective?.protocol.value === "serial" &&
    effective.serial.port === null
    ? NO_PORT
    : null;
}

/** The other ways in (all three while `effective` is not known yet). */
export function otherWays(effective: EffectiveSettings | null): WayIn[] {
  const noSerialPort = effective !== null && effective.serial.port === null;
  return (["ssh", "telnet", "serial"] as const)
    .filter((protocol) => protocol !== effective?.protocol.value)
    .map((protocol) => ({
      protocol,
      blocked: protocol === "serial" && noSerialPort ? NO_PORT : null,
    }));
}

// How a workspace host is connected from the palette or the tree, in
// whichever window (#88): the main window and a session window decide it
// alike.
import type { HostLogin } from "./credentials";
import type { ConnectTarget } from "./session";
import type { Host } from "./workspace";

/** A host that connects at once: its target, its header's title, and
 * the username of the credential profile it logs in with (null: none). */
export interface HostConnection {
  target: ConnectTarget;
  title: string;
  username: string | null;
}

/**
 * How `host` connects, given its `login`: over telnet or serial at once
 * (the device asks for the login in the terminal), over SSH at once with
 * its credential profile. Null when its credentials must be asked first
 * (or its profile set up), which only the main window does.
 */
export function hostConnection(
  host: Host,
  login: HostLogin,
): HostConnection | null {
  switch (login.kind) {
    case "telnet":
      return {
        target: { kind: "telnet", id: host.id },
        title: `${host.name} (telnet)`,
        username: null,
      };
    case "serial":
      return {
        target: { kind: "serial", id: host.id },
        title: `${host.name} (serial)`,
        username: null,
      };
    case "profile":
      return {
        target: { kind: "host", id: host.id, credentials: null },
        title: `${login.username}@${host.name}`,
        username: login.username,
      };
    default:
      return null;
  }
}

/** The host's address, for a header whose title does not show it. */
export function addressBesides(host: Host, title: string): string | null {
  return host.address && !title.includes(host.address) ? host.address : null;
}

#!/usr/bin/python3
"""Pointer and keyboard input for a GNOME Shell session (#58), through
Mutter's RemoteDesktop D-Bus API: the path GNOME's own remote desktop
takes, so the compositor sees real input from a seat.

Used by scripts/linux-desktop-session.mjs. Reads commands on stdin, one per
line, and answers each with a line starting "ok" (or "error ..."):

    to <x> <y>        move the pointer to (x, y) on the screen
    button <down|up>  the left button
    key <keysym> <down|up>

A RemoteDesktop session lives as long as the D-Bus connection that made it,
so this process keeps it open. Absolute motion needs a screen cast stream
(and so PipeWire); relative motion does not, so "to" moves relatively from
where GNOME Shell says the pointer is (its Eval, in --unsafe-mode).
"""

import sys
import time

from gi.repository import Gio, GLib

BTN_LEFT = 0x110
BUS = Gio.bus_get_sync(Gio.BusType.SESSION)
DESKTOP = "org.gnome.Mutter.RemoteDesktop"
SESSION = "org.gnome.Mutter.RemoteDesktop.Session"


def call(destination, path, interface, method, arguments=None):
    return BUS.call_sync(destination, path, interface, method, arguments,
                         None, Gio.DBusCallFlags.NONE, -1, None)


def pointer():
    found = call("org.gnome.Shell", "/org/gnome/Shell", "org.gnome.Shell",
                 "Eval",
                 GLib.Variant("(s)", ("global.get_pointer().slice(0, 2)",)))
    ok, answer = found.unpack()
    if not ok:
        raise RuntimeError(f"Eval refused (is GNOME Shell in --unsafe-mode?): {answer}")
    x, y = answer.strip("[]").split(",")
    return float(x), float(y)


session = call(DESKTOP, "/org/gnome/Mutter/RemoteDesktop", DESKTOP,
               "CreateSession").unpack()[0]
call(DESKTOP, session, SESSION, "Start")
print("ok ready", flush=True)

for line in sys.stdin:
    words = line.split()
    if not words:
        continue
    try:
        if words[0] == "to":
            x, y = float(words[1]), float(words[2])
            # The shell may clamp or apply a move a moment later: move
            # until it reports the place.
            for _ in range(20):
                at_x, at_y = pointer()
                if abs(at_x - x) < 1 and abs(at_y - y) < 1:
                    break
                call(DESKTOP, session, SESSION, "NotifyPointerMotionRelative",
                     GLib.Variant("(dd)", (x - at_x, y - at_y)))
                time.sleep(0.05)
            else:
                raise RuntimeError(f"the pointer did not reach {x} {y}")
        elif words[0] == "button":
            call(DESKTOP, session, SESSION, "NotifyPointerButton",
                 GLib.Variant("(ib)", (BTN_LEFT, words[1] == "down")))
        elif words[0] == "key":
            call(DESKTOP, session, SESSION, "NotifyKeyboardKeysym",
                 GLib.Variant("(ub)", (int(words[1], 0), words[2] == "down")))
        else:
            raise RuntimeError(f"unknown command {words[0]}")
        print("ok", flush=True)
    except Exception as error:  # noqa: BLE001 - reported to the caller
        print(f"error {error}", flush=True)

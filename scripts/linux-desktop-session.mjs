// A real desktop session out of sight, for `npm run test:linux-desktop --
// --desktop xfce|gnome` (#58). Nothing of it reaches the screen of the
// computer the check runs on: under WSL the session gets a runtime folder
// of its own, so WSLg's Wayland and X sockets are out of its reach.
//
// - `xfce`: an X11 session on a virtual display (Xvfb) with XFCE's window
//   manager (xfwm4), its panel holding the Whisker menu and a task list,
//   its settings daemon (xfsettingsd, which serves the GTK theme over
//   XSETTINGS) and the desktop portal. Input is X's own (xdotool, XTEST);
//   the window manager's view of a window is read with xdotool and xprop.
// - `gnome`: GNOME Shell on Wayland, headless with a virtual monitor (its
//   own compositor, Mutter), in --unsafe-mode so that its Eval can read
//   the windows as the shell has them. Input goes through Mutter's
//   RemoteDesktop API (scripts/linux-desktop-input.py), as GNOME's remote
//   desktop does; pictures through its Screenshot API.
//
// Each has a D-Bus session bus and XDG folders of its own (in `folder`),
// so its settings (dconf, xfconf, the default applications) are the
// check's, never the user's.
//
// Needs (Ubuntu): xvfb xfwm4 xfce4-panel xfce4-settings
// xfce4-whiskermenu-plugin xdotool x11-utils imagemagick for xfce;
// gnome-shell for gnome; python3-gi and xdg-desktop-portal-gtk for both.

import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const SCREEN = { width: 1600, height: 1000 };
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
/** A point kept on the screen (a pointer cannot leave it). */
const onScreen = (x, y) => [
  Math.min(Math.max(Math.round(x), 0), SCREEN.width - 1),
  Math.min(Math.max(Math.round(y), 0), SCREEN.height - 1),
];

/** Runs `command` to the end; its output, or a thrown error. */
function run(command, args, env, { allowFailure = false } = {}) {
  const done = spawnSync(command, args, { env, encoding: "utf8" });
  if (done.error) throw new Error(`${command}: ${done.error.message}`);
  if (done.status !== 0 && !allowFailure)
    throw new Error(
      `${command} ${args.join(" ")}: ${(done.stderr || done.stdout).trim()}`,
    );
  return done.stdout;
}

async function waitFor(description, probe, timeout = 30000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await probe();
      if (last) return last;
    } catch (error) {
      last = error;
    }
    await sleep(200);
  }
  throw new Error(
    `the desktop: timed out waiting for ${description} (${last})`,
  );
}

/** The environment shared by everything in the session: none of WSLg's
 * sockets, folders of the session's own. */
function baseEnvironment(folder) {
  const env = { ...process.env };
  for (const name of [
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "GDK_BACKEND",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "DESKTOP_SESSION",
    "GTK_THEME",
    // WSLg's sound server, on the Windows side.
    "PULSE_SERVER",
  ])
    delete env[name];
  for (const [name, sub] of [
    ["XDG_RUNTIME_DIR", "run"],
    ["XDG_CONFIG_HOME", "config"],
    ["XDG_DATA_HOME", "data"],
    ["XDG_CACHE_HOME", "cache"],
  ]) {
    env[name] = join(folder, sub);
    mkdirSync(env[name], { recursive: true, mode: 0o700 });
  }
  return env;
}

/** The default applications a user picks: a file manager for folders,
 * a browser for web pages (the freedesktop list every opener reads, and
 * XFCE's own, which its `exo-open` reads). */
function defaultApplications(env, { fileManager, browser }) {
  writeFileSync(
    join(env.XDG_CONFIG_HOME, "mimeapps.list"),
    [
      "[Default Applications]",
      `inode/directory=${fileManager.desktop}`,
      `x-scheme-handler/http=${browser.desktop}`,
      `x-scheme-handler/https=${browser.desktop}`,
      `text/html=${browser.desktop}`,
      "",
    ].join("\n"),
  );
  mkdirSync(join(env.XDG_CONFIG_HOME, "xfce4"), { recursive: true });
  writeFileSync(
    join(env.XDG_CONFIG_HOME, "xfce4", "helpers.rc"),
    `WebBrowser=${browser.helper}\nFileManager=${fileManager.helper}\n`,
  );
}

export const FILE_MANAGER = {
  desktop: "thunar.desktop",
  helper: "thunar",
  classes: ["Thunar", "thunar"],
};
export const BROWSER = {
  desktop: "org.gnome.Epiphany.desktop",
  helper: "epiphany",
  classes: ["Epiphany", "epiphany", "org.gnome.Epiphany"],
};

function startChild(command, args, env, children, log) {
  const child = spawn(command, args, {
    env,
    stdio: ["ignore", "pipe", "pipe"],
    detached: true,
  });
  let output = "";
  const keep = (chunk) => {
    output = (output + chunk).slice(-20000);
  };
  child.stdout.on("data", keep);
  child.stderr.on("data", keep);
  child.output = () => output;
  child.on("error", (error) => log(`${command}: ${error.message}`));
  children.push(child);
  return child;
}

/** A D-Bus session bus of the session's own. */
async function startBus(env, children, log) {
  const bus = startChild(
    "dbus-daemon",
    ["--session", "--nofork", "--nopidfile", "--print-address=1"],
    env,
    children,
    log,
  );
  const address = await waitFor(
    "the D-Bus session bus",
    () => bus.output().match(/^(unix:\S+)/m)?.[1],
  );
  env.DBUS_SESSION_BUS_ADDRESS = address;
}

/** Ends the children, newest first, and their process groups. */
function stopAll(children) {
  for (const child of [...children].reverse()) {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      try {
        child.kill();
      } catch {
        // Gone already.
      }
    }
  }
}

// --- XFCE on X11 --------------------------------------------------------

/** A display number no X server has: no lock file, no socket (a file or
 * an abstract one). */
function freeDisplay() {
  const sockets = readFileSync("/proc/net/unix", "utf8");
  for (let number = 58; number < 99; number++)
    if (
      !existsSync(`/tmp/.X${number}-lock`) &&
      !sockets.includes(`/tmp/.X11-unix/X${number}\n`)
    )
      return number;
  throw new Error("no free X display number from :58 to :98");
}

/** The panel of a plain XFCE desktop: the Whisker menu (Xubuntu's), the
 * task list and a clock, along the top. */
const XFCE_PANEL = `<?xml version="1.0" encoding="UTF-8"?>
<channel name="xfce4-panel" version="1.0">
  <property name="configver" type="int" value="2"/>
  <property name="panels" type="array">
    <value type="int" value="1"/>
    <property name="panel-1" type="empty">
      <property name="position" type="string" value="p=6;x=0;y=0"/>
      <property name="length" type="uint" value="100"/>
      <property name="position-locked" type="bool" value="true"/>
      <property name="size" type="uint" value="30"/>
      <property name="plugin-ids" type="array">
        <value type="int" value="1"/>
        <value type="int" value="2"/>
        <value type="int" value="3"/>
        <value type="int" value="4"/>
      </property>
    </property>
  </property>
  <property name="plugins" type="empty">
    <property name="plugin-1" type="string" value="whiskermenu"/>
    <property name="plugin-2" type="string" value="tasklist"/>
    <property name="plugin-3" type="string" value="separator">
      <property name="expand" type="bool" value="true"/>
    </property>
    <property name="plugin-4" type="string" value="clock"/>
  </property>
</channel>
`;

async function startXfce(env, children, log) {
  env.GDK_BACKEND = "x11";
  env.XDG_SESSION_TYPE = "x11";
  env.XDG_CURRENT_DESKTOP = "XFCE";
  env.XDG_SESSION_DESKTOP = "xfce";
  const channels = join(
    env.XDG_CONFIG_HOME,
    "xfce4",
    "xfconf",
    "xfce-perchannel-xml",
  );
  mkdirSync(channels, { recursive: true });
  writeFileSync(join(channels, "xfce4-panel.xml"), XFCE_PANEL);
  // xfwm4 wraps a window dragged against the screen's edge to the next
  // workspace by default, and then tiles none (Thunar's neither): turned
  // off, as people who tile do, dragging to the edge tiles.
  writeFileSync(
    join(channels, "xfwm4.xml"),
    `<?xml version="1.0" encoding="UTF-8"?>
<channel name="xfwm4" version="1.0">
  <property name="general" type="empty">
    <property name="wrap_windows" type="bool" value="false"/>
    <property name="tile_on_move" type="bool" value="true"/>
  </property>
</channel>
`,
  );
  // -displayfd: Xvfb names the display once it takes clients. (Under WSL
  // /tmp/.X11-unix is WSLg's and not writable: Xvfb then listens on the
  // abstract socket only, which clients try first.)
  const server = startChild(
    "Xvfb",
    [
      `:${freeDisplay()}`,
      "-displayfd",
      "1",
      "-screen",
      "0",
      `${SCREEN.width}x${SCREEN.height}x24`,
      "-nolisten",
      "tcp",
    ],
    env,
    children,
    log,
  );
  const number = await waitFor("the virtual display", () => {
    if (server.exitCode !== null)
      throw new Error(`Xvfb exited: ${server.output().slice(-800)}`);
    return server.output().match(/^(\d+)$/m)?.[1];
  });
  env.DISPLAY = `:${number}`;
  await startBus(env, children, log);
  startChild("xfsettingsd", ["--replace"], env, children, log);
  startChild("xfwm4", ["--replace"], env, children, log);
  await waitFor("the window manager", () =>
    run("wmctrl", ["-m"], env, { allowFailure: true }).includes("Xfwm4"),
  );
  startChild("xfce4-panel", [], env, children, log);
  await waitFor("the panel", () =>
    run("xdotool", ["search", "--class", "xfce4-panel"], env, {
      allowFailure: true,
    }).trim(),
  );
  // The menu is a plugin the panel starts on its own, a moment later.
  await waitFor("the Whisker menu plugin", () =>
    run("pgrep", ["-f", "plugins/libwhiskermenu.so"], env, {
      allowFailure: true,
    }).trim(),
  );
  await sleep(1500);

  const xdotool = (...args) => run("xdotool", args.map(String), env);
  const state = (id) =>
    run("xprop", ["-id", id, "_NET_WM_STATE", "WM_STATE"], env, {
      allowFailure: true,
    });
  const desktop = {
    kind: "xfce",
    name: "XFCE (xfwm4) on X11 (Xvfb)",
    /** The windows of the given classes, as the window manager has them
     * (minimized ones too). */
    windows(classes) {
      const found = new Map();
      // The client windows the window manager handles (its client list),
      // of the classes asked for.
      const managed =
        run("xprop", ["-root", "_NET_CLIENT_LIST"], env, {
          allowFailure: true,
        }).match(/0x[0-9a-f]+/g) ?? [];
      for (const id of managed.map((hex) => String(parseInt(hex, 16)))) {
        {
          const names =
            run("xprop", ["-id", id, "WM_CLASS"], env, {
              allowFailure: true,
            }).match(/"([^"]*)"/g) ?? [];
          if (!names.some((name) => classes.includes(name.slice(1, -1))))
            continue;
          const wmState = state(id);
          const shell = run(
            "xdotool",
            ["getwindowgeometry", "--shell", id],
            env,
            { allowFailure: true },
          );
          const value = (key) =>
            Number(shell.match(new RegExp(`${key}=(-?\\d+)`))?.[1]);
          const pid = Number(
            run("xdotool", ["getwindowpid", id], env, {
              allowFailure: true,
            }).trim(),
          );
          const title = run("xdotool", ["getwindowname", id], env, {
            allowFailure: true,
          }).trim();
          found.set(id, {
            id,
            pid,
            title,
            x: value("X"),
            y: value("Y"),
            width: value("WIDTH"),
            height: value("HEIGHT"),
            minimized:
              /_NET_WM_STATE_HIDDEN/.test(wmState) ||
              /window state: Iconic/.test(wmState),
            maximized:
              /_NET_WM_STATE_MAXIMIZED_VERT/.test(wmState) &&
              /_NET_WM_STATE_MAXIMIZED_HORZ/.test(wmState),
          });
        }
      }
      return [...found.values()];
    },
    /** Where windows may go (the screen less the panel). */
    workArea() {
      const [x, y, width, height] = run(
        "xprop",
        ["-root", "_NET_WORKAREA"],
        env,
      )
        .split("=")[1]
        .split(",")
        .map(Number);
      return { x, y, width, height };
    },
    async to(x, y) {
      const [atX, atY] = onScreen(x, y);
      xdotool("mousemove", atX, atY);
      await sleep(60);
    },
    async down() {
      xdotool("mousedown", 1);
      await sleep(60);
    },
    async up() {
      xdotool("mouseup", 1);
      await sleep(60);
    },
    async type(text) {
      xdotool("type", "--delay", 40, text);
    },
    async key(name) {
      xdotool("key", name);
    },
    /** Brings a window back as the task list does (_NET_ACTIVE_WINDOW). */
    activate(window) {
      xdotool("windowactivate", window.id);
    },
    close(window) {
      run("wmctrl", ["-i", "-c", window.id], env, { allowFailure: true });
    },
    minimize(window) {
      xdotool("windowminimize", window.id);
    },
    /** Puts a window somewhere (to recover after a failed step). */
    place(window, x, y, width, height) {
      if (width) xdotool("windowsize", window.id, width, height);
      xdotool("windowmove", window.id, x, y);
    },
    /** The key that tiles the active window to the left half. */
    tileLeftKey: "super+KP_Left",
    picture(file) {
      run("import", ["-window", "root", file], env);
    },
    /** XFCE's own switch (Settings, Appearance, Style): the GTK theme,
     * served to the apps over XSETTINGS by xfsettingsd. */
    setDark(dark) {
      run(
        "xfconf-query",
        [
          "-c",
          "xsettings",
          "-p",
          "/Net/ThemeName",
          "-n",
          "-t",
          "string",
          "-s",
          dark ? "Adwaita-dark" : "Adwaita",
        ],
        env,
      );
      return `xfconf xsettings /Net/ThemeName = ${dark ? "Adwaita-dark" : "Adwaita"}`;
    },
    /** Opens the Whisker menu and searches for `name`; Return launches the
     * first hit. */
    async searchMenu(name) {
      run("xfce4-popup-whiskermenu", [], env);
      // The menu opens with its search field focused.
      await sleep(1500);
      await desktop.type(name);
      await sleep(1000);
    },
    async launchMenuHit() {
      await desktop.key("Return");
    },
    /** What the session's window manager calls the app's window. */
  };
  return desktop;
}

// --- GNOME Shell on Wayland ----------------------------------------------

async function startGnome(env, children, log) {
  const socket = "netterm-check-wayland";
  env.GDK_BACKEND = "wayland";
  env.XDG_SESSION_TYPE = "wayland";
  env.XDG_CURRENT_DESKTOP = "GNOME";
  env.XDG_SESSION_DESKTOP = "gnome";
  await startBus(env, children, log);
  const gsettings = (...args) => run("gsettings", args, env);
  gsettings("set", "org.gnome.desktop.interface", "enable-animations", "false");
  // The pointer passing the top-left corner would open the overview.
  gsettings(
    "set",
    "org.gnome.desktop.interface",
    "enable-hot-corners",
    "false",
  );
  gsettings("set", "org.gnome.desktop.interface", "color-scheme", "default");
  const shell = startChild(
    "gnome-shell",
    [
      "--headless",
      "--wayland",
      "--no-x11",
      "--unsafe-mode",
      "--virtual-monitor",
      `${SCREEN.width}x${SCREEN.height}`,
      "--wayland-display",
      socket,
    ],
    env,
    children,
    log,
  );
  await waitFor("GNOME Shell's Wayland socket", () => {
    if (shell.exitCode !== null)
      throw new Error(`gnome-shell exited: ${shell.output().slice(-800)}`);
    return existsSync(join(env.XDG_RUNTIME_DIR, socket));
  });
  env.WAYLAND_DISPLAY = socket;

  const evaluate = (code) => {
    const answer = run(
      "gdbus",
      [
        "call",
        "--session",
        "--dest",
        "org.gnome.Shell",
        "--object-path",
        "/org/gnome/Shell",
        "--method",
        "org.gnome.Shell.Eval",
        `(() => { ${code} })()`,
      ],
      env,
    ).trim();
    // (true, '<JSON of the value>') as GVariant text: in single quotes,
    // with quotes and backslashes escaped; "" for undefined.
    const match = answer.match(/^\((true|false), (['"])(.*)\2\)$/s);
    if (!match) throw new Error(`Eval: ${answer}`);
    const text = match[3].replace(/\\(.)/g, "$1");
    if (match[1] !== "true") throw new Error(`Eval failed: ${text}`);
    return text === "" ? undefined : JSON.parse(text);
  };
  await waitFor("GNOME Shell's Eval", () => evaluate("return 1") === 1);
  // GNOME starts in the overview; a user's first click leaves it. Hidden
  // while the shell is still starting, the overview keeps the pointer's
  // buttons from every window.
  await waitFor(
    "GNOME Shell started",
    () => evaluate("return !Main.layoutManager._startingUp"),
    60000,
  );
  await sleep(2000);
  await waitFor("the overview left", () =>
    evaluate("Main.overview.hide(); return !Main.overview.visible"),
  );

  const input = spawn("python3", [join(HERE, "linux-desktop-input.py")], {
    env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  children.push(input);
  let inputErrors = "";
  input.stderr.on("data", (chunk) => (inputErrors += chunk));
  const answers = createInterface({ input: input.stdout });
  const pending = [];
  let ready = false;
  answers.on("line", (line) => {
    if (!ready) ready = line === "ok ready";
    else pending.shift()?.(line);
  });
  await waitFor("the RemoteDesktop session", () => {
    if (input.exitCode !== null)
      throw new Error(`the input helper exited: ${inputErrors}`);
    return ready;
  });
  const ask = (line) =>
    new Promise((resolve, reject) => {
      pending.push((answer) =>
        answer.startsWith("ok")
          ? resolve(answer)
          : reject(new Error(`input: ${line}: ${answer}`)),
      );
      input.stdin.write(`${line}\n`);
    });
  // The overview opened once with a click on Activities, as a user does:
  // until then (the overview of the start hidden by Eval) the shell hands
  // no pointer button to the windows.
  await ask("to 30 15");
  await ask("button down");
  await ask("button up");
  await waitFor("the overview shown by Activities", () =>
    evaluate("return Main.overview.visible"),
  );
  await waitFor("the overview left", () =>
    evaluate("Main.overview.hide(); return !Main.overview.visible"),
  );

  const keysyms = {
    Return: 0xff0d,
    Super_L: 0xffeb,
    Left: 0xff51,
    Escape: 0xff1b,
  };
  const windowsCode = (classes) => `
    const wanted = ${JSON.stringify(classes)};
    const tracker = imports.gi.Shell.WindowTracker.get_default();
    return global.get_window_actors().map((actor) => actor.meta_window)
      .filter((w) => wanted.includes(w.get_wm_class()))
      .map((w) => {
        const r = w.get_frame_rect();
        const flags = w.get_maximize_flags ? w.get_maximize_flags() : w.get_maximized();
        return { id: String(w.get_id()), pid: w.get_pid(), title: w.get_title(),
          x: r.x, y: r.y, width: r.width, height: r.height,
          minimized: w.minimized, maximized: flags === 3,
          app: tracker.get_window_app(w)?.get_id() ?? null };
      });`;
  const desktop = {
    kind: "gnome",
    name: "GNOME Shell (Mutter) on Wayland, headless",
    evaluate,
    windows: (classes) => evaluate(windowsCode(classes)),
    workArea: () =>
      evaluate(
        "const a = Main.layoutManager.getWorkAreaForMonitor(0); return { x: a.x, y: a.y, width: a.width, height: a.height };",
      ),
    to: (x, y) => ask(`to ${onScreen(x, y).join(" ")}`),
    down: () => ask("button down"),
    up: () => ask("button up"),
    async type(text) {
      for (const char of text) {
        const code = char.codePointAt(0);
        await ask(`key ${code} down`);
        await ask(`key ${code} up`);
      }
    },
    async key(name) {
      const parts = name
        .split("+")
        .map((part) => keysyms[part] ?? part.codePointAt(0));
      for (const code of parts) await ask(`key ${code} down`);
      for (const code of [...parts].reverse()) await ask(`key ${code} up`);
    },
    /** Brings a window back as the dock and the window switcher do. */
    activate: (window) =>
      evaluate(
        `const w = global.get_window_actors().map((a) => a.meta_window).find((w) => String(w.get_id()) === ${JSON.stringify(window.id)}); Main.activateWindow(w); return true;`,
      ),
    place: (window, x, y, width, height) =>
      evaluate(
        `const w = global.get_window_actors().map((a) => a.meta_window).find((w) => String(w.get_id()) === ${JSON.stringify(window.id)});
         ${width ? `w?.move_resize_frame(true, ${x}, ${y}, ${width}, ${height});` : `w?.move_frame(true, ${x}, ${y});`}
         return true;`,
      ),
    tileLeftKey: "Super_L+Left",
    /** The windows shown, not the overview. */
    async calm() {
      await waitFor("the overview left", () =>
        evaluate("Main.overview.hide(); return !Main.overview.visible"),
      );
    },
    minimize: (window) =>
      evaluate(
        `global.get_window_actors().map((a) => a.meta_window).find((w) => String(w.get_id()) === ${JSON.stringify(window.id)})?.minimize(); return true;`,
      ),
    close: (window) =>
      evaluate(
        `const w = global.get_window_actors().map((a) => a.meta_window).find((w) => String(w.get_id()) === ${JSON.stringify(window.id)}); w?.delete(global.get_current_time()); return true;`,
      ),
    picture(file) {
      const shot = join(env.XDG_RUNTIME_DIR, "screenshot.png");
      run(
        "gdbus",
        [
          "call",
          "--session",
          "--dest",
          "org.gnome.Shell.Screenshot",
          "--object-path",
          "/org/gnome/Shell/Screenshot",
          "--method",
          "org.gnome.Shell.Screenshot.Screenshot",
          "false",
          "false",
          shot,
        ],
        env,
      );
      writeFileSync(file, readFileSync(shot));
    },
    /** GNOME's own switch (Settings, Appearance): the colour scheme,
     * which the desktop portal announces to apps. */
    setDark(dark) {
      gsettings(
        "set",
        "org.gnome.desktop.interface",
        "color-scheme",
        dark ? "prefer-dark" : "default",
      );
      return `gsettings org.gnome.desktop.interface color-scheme = ${dark ? "prefer-dark" : "default"}`;
    },
    /** The overview's search, as Super then typing does. */
    async searchMenu(name) {
      evaluate("Main.overview.show(); return true");
      await waitFor("the overview", () =>
        evaluate("return Main.overview.visible"),
      );
      await sleep(500);
      await desktop.type(name);
      await sleep(1500);
    },
    async launchMenuHit() {
      await desktop.key("Return");
      await sleep(500);
      evaluate("Main.overview.hide(); return true");
    },
    /** The menu's entry for `id` as GNOME's app system has it. */
    menuEntry: (id) =>
      evaluate(`
        const app = imports.gi.Shell.AppSystem.get_default().lookup_app(${JSON.stringify(id)});
        return app && { name: app.get_name(), icon: app.get_icon()?.to_string() ?? null,
          shown: app.get_app_info().should_show() };`),
  };
  return desktop;
}

/** Starts a desktop session of `kind` ("xfce" or "gnome") in `folder`.
 * Resolves to its helpers; `env` is the environment for what runs in it,
 * and `stop()` ends it. */
export async function startDesktop(kind, folder, log = console.log) {
  mkdirSync(folder, { recursive: true });
  const env = baseEnvironment(folder);
  defaultApplications(env, { fileManager: FILE_MANAGER, browser: BROWSER });
  const children = [];
  try {
    const desktop =
      kind === "xfce"
        ? await startXfce(env, children, log)
        : kind === "gnome"
          ? await startGnome(env, children, log)
          : null;
    if (!desktop) throw new Error(`unknown desktop ${kind} (xfce or gnome)`);
    desktop.env = env;
    desktop.stop = () => {
      stopAll(children);
      // And what was started in the session and left it (a file manager
      // or browser an opener started): every process of this user whose
      // environment names the session's runtime folder.
      const mark = `XDG_RUNTIME_DIR=${env.XDG_RUNTIME_DIR}\0`;
      for (const pid of readdirSync("/proc").filter((name) =>
        /^\d+$/.test(name),
      ))
        try {
          if (
            Number(pid) !== process.pid &&
            readFileSync(`/proc/${pid}/environ`, "utf8").includes(mark)
          )
            process.kill(Number(pid), "SIGKILL");
        } catch {
          // Not ours, or gone.
        }
      // The document portal mounts a FUSE folder in the runtime folder.
      run("fusermount", ["-u", join(env.XDG_RUNTIME_DIR, "doc")], env, {
        allowFailure: true,
      });
    };
    return desktop;
  } catch (error) {
    stopAll(children);
    throw error;
  }
}

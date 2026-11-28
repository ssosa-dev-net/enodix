# Contributing to enodix

Thank you for helping. Bug reports, ideas and pull requests are all welcome.

## Reporting a problem or an idea

Open an issue. For a problem, say what you did, what you expected and what
happened, with the enodix version, the operating system and, for a device
session, the device's platform and software version. Leave out passwords,
keys, addresses and configurations you would not publish.

## Building from source

You need Node.js, a Rust toolchain and the
[Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) for your
system. Then, from a clone:

```sh
npm install          # JavaScript dependencies
npm run app:dev      # run the app with live reload
npm run check        # format check, lint and every test
npm run app:build    # build the app, without installers
```

`npm run test:python` runs the Python bridge tests and `npm run test:linux`
the Linux checks. The maintainer pages under [docs/dev](docs/dev) cover the
[containerlab device suite](docs/dev/containerlab.md), the
[Linux checks](docs/dev/linux.md) and [making a release](docs/dev/releasing.md).
[docs/demo-workspace](docs/demo-workspace) is the example network the
screenshots in the documentation come from.

## Pull requests

1. Talk about larger changes in an issue first, so the work fits the plan.
2. Build and test from a clone: `npm install`, then `npm run check` (format
   check, lint and every test) must pass. `npm run test:python` covers the
   Python bridge and `npm run test:linux` the Linux checks. CI runs the
   same checks on Linux and Windows for every pull request
   (`.github/workflows/ci.yml`); a pull request is merged once they pass.
   The end-to-end smoke test (`scripts/smoke.mjs`) runs there too, as the
   "Smoke test (Windows)" job, and must pass. A pull request that adds or
   changes a screen adds its smoke steps in the same pull request: the
   screen is not covered, and the pull request not ready, until the test
   drives it. The containerlab device suite and the installer checks need
   a lab or Windows Sandbox, so the maintainer runs them before a release. A new or updated dependency must
   also pass `cargo deny check` (known vulnerabilities, licences and
   sources, set in `deny.toml`) and `npm audit --omit=dev`; CI runs both
   (`.github/workflows/dependencies.yml`), and again every week.
3. Tests check what a user would see, through `enodix-core`'s public API;
   extend the test harness in `crates/core/tests/support` rather than
   mocking internals.
4. Keep each pull request to one change, and say in its description what
   it changes and how you checked it.
5. Agree to the Contributor Licence Agreement below, by ticking its box in
   the pull request (once per pull request).

## Contributor Licence Agreement

enodix is published under the GNU General Public License, version 3. Its
author, Sergio Sosa ("the maintainer"), also wants to keep the freedom to
offer it under other terms later, for example a commercial licence for a
company that cannot use the GPL. This agreement is what makes that possible
for code you contribute. It does not take your copyright away: you keep it,
and you can use your contribution however you like.

By submitting a contribution (any code, documentation or other material you
send to this project, through a pull request or otherwise), you agree that:

1. **Copyright licence.** You grant the maintainer, and anyone the
   maintainer licenses enodix to, a perpetual, worldwide, non-exclusive,
   royalty-free, irrevocable licence to use, copy, modify, publish,
   distribute, sublicense and relicense your contribution, under the GPL or
   any other licence, open source or not.
2. **Patent licence.** If you hold a patent that your contribution
   infringes, you grant the same people a perpetual, worldwide,
   non-exclusive, royalty-free, irrevocable licence under it to make, use,
   sell and distribute your contribution as part of enodix.
3. **It is yours to give.** The contribution is your own original work, or
   you have the right to submit it under these terms and you say where it
   came from. If your employer or anyone else has rights in work you make,
   they have allowed this contribution, or have waived those rights.
4. **No promise either way.** The maintainer does not have to use your
   contribution. You give it as it is, with no warranty, and you do not
   have to support it.
5. **It stays open.** Whatever else happens, every contribution that is
   included in enodix remains available under the GNU General Public
   License, version 3, or a later version of it, or another licence the
   Open Source Initiative approves.

If you cannot agree to this, say so in an issue before sending the work, and
we will find another way.

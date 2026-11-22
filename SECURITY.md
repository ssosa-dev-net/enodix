# Security policy

enodix logs in to network devices and keeps credentials (in the system's
credential store or its own encrypted vault), so security problems matter.
Thank you for reporting one.

## Reporting a vulnerability

**Please do not open a public issue.** Report it privately through GitHub:
on the repository's **Security** tab, choose **Report a vulnerability**.
Only the maintainer sees the report.

Include what you can of:

- what the problem is and what an attacker could do with it;
- the enodix version and operating system;
- the steps to reproduce it, or a proof of concept;
- whether it needs a device, a jump host or a particular setting.

Leave out real passwords, keys and device addresses; made-up ones are fine.

## What to expect

- An acknowledgement within a week.
- An assessment and, for a confirmed problem, a plan for a fix, kept
  in the private report.
- A fixed release, and a published security advisory crediting you unless
  you prefer otherwise. Please keep the problem private until then.

enodix is maintained by one person in their own time, so these are aims,
not guarantees.

## Supported versions

Fixes go into the newest release only. enodix is still before 1.0: update
to the latest version before reporting, if you can.

## In scope

The app, `enodix-cli`, the `enodix` Python package and the installers and
packages published on this repository's releases: for example how
credentials, the vault, host keys, session logs and the connections the app
makes are handled. Weaknesses of the devices you connect to, or of
legacy algorithms you turn on yourself for an old device, are not
vulnerabilities in enodix.

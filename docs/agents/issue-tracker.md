# Issue tracker: GitHub Issues

Issues and specs for this repo live as GitHub Issues in the repository that `origin` points at. Use the `gh` CLI for all operations.

## Setup (once per machine, done by the user)

- `gh` is installed and logged in (`gh auth status`).
- Agents never create tokens or type them; if `gh` is missing or not logged in, stop and tell the user.

## Conventions

Run `gh` inside the clone; it finds the repository from the git remote.

- **Create an issue**: `gh issue create --title "..." --body-file <file> --label "a,b"`. Write the body to a file in the scratchpad with the file tools; do not use heredocs (see `CLAUDE.md`). `--body "..."` is fine for one line.
- **Read an issue**: `gh issue view <number> --comments`
- **List issues**: `gh issue list --state open --json number,title,labels`, with `--label "..."` and `--state open|closed|all` as needed.
- **Comment on an issue**: `gh issue comment <number> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --add-label "..."` / `--remove-label "..."`
- **Create a missing label**: `gh label create "..." --color rrggbb`
- **Close**: comment first, then `gh issue close <number>`

Issue numbers are written `#<number>` in commit messages and in `CLAUDE.md`. The numbers up to the move to GitHub (in the changelog, the roadmap and the area notes) are those of the project's earlier tracker and do not match GitHub's.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests; `/triage` reads this flag.)_

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a single issue with **child** issues as tickets.

- **Map**: a single issue labelled `wayfinder:map`, holding the Notes / Decisions-so-far / Fog body.
- **Child ticket**: an issue with `Part of #<map>` at the top of its body, also listed in a task list in the map body. Labels: `wayfinder:<type>` (`research`/`prototype`/`grilling`/`task`).
- **Blocking**: a `Blocked by: #<n>, #<n>` line at the top of the child body. A ticket is unblocked when every issue it lists is closed.
- **Frontier query**: list the map's open children, drop any with an open blocker or an assignee; first in map order wins.
- **Claim**: assign the issue to the driving developer (`gh issue edit <n> --add-assignee <user>`), the session's first write.
- **Resolve**: comment with the answer, close the issue, then append a context pointer to the map's Decisions-so-far.

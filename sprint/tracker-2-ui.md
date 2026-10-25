# Tracker sprint 2: web UI and its own notifications

Status: **UI built 2026-10-24, firmware built, not yet flashed** (ops.local
was not on USB). The Angular project is `microcontroller/miniframework/web/projects/tracker`.
Its 6 logic tests pass under vitest (19 in the workspace), `ng build`
passes strict templates, and the bundle is 81 KiB gzipped. The firmware is
1.66 MB, 52% of the app slot.

## As built

- Pages: **Issues** (open/closed/all, search, priority, label chips with
  open counts; the filter lives in the URL so a filtered list can be
  shared), **Issue** (meta, body, history with comments, repeat reports and
  events; triage tools to close with a resolution, reopen, set priority,
  labels and assignee; a comment box), **New issue**, and **Tokens** (admin:
  create, with the secret shown once; revoke).
- Sign-in: one field for the admin password or a token, kept for the tab
  (sessionStorage). The new `GET /api/v1/whoami` returns `{name, scope}`, and
  the UI shows only the actions that scope allows.
- Bodies and comments render as text, never HTML: the reporters are bots and boards.
- Make: `tracker-web`, `tracker-web-dev` (proxy to :8090, or
  `TRACKER=https://ops.local`), `run-tracker-bundle`. `tracker-firmware` now
  builds the UI first.
- The SRE bot's notices now link to the issue page: "Issue #12 https://ops.local/issues/12".
- Not done: tracker webhooks and email (item 3 below), and markdown rendering.

## Plan (as written before building)

Status: planned. After tracker sprint 1.

## Goals

1. **Angular UI**, like the other board UIs and served from the board:
   - Issue list with filters (state, label, assignee, priority, text), sorted
     by `last_seen` or updated.
   - Issue page with body, comments, history, occurrence count and timeline;
     edit labels, assignee, priority and state; comment.
   - Token admin: create (shown once), scope, revoke, last used.
   - Labels admin.
2. **Human login**: see the overview's open question (household trust vs. local users).
3. **Generic notifications from the tracker**: an outgoing webhook per
   subscription (event types: opened, reopened, commented, closed; label
   filter). The SRE bot doesn't need this; it's for any other client. Email
   can come later, by reusing the SMTP code from SRE sprint 2 if it moves into
   a shared crate. Decide in sprint.

## Tests

- `ng test` specs for the list, detail and token pages.
- An e2e test against the desktop tracker: file an issue via the API, see it
  in the UI, comment, close.

# Website architecture: server HTML and browser application

Planning report, 2026-09-26. Recommendation only; no implementation changes.
Based on the current working tree, including existing uncommitted work. The
bots projects are outside this review. This is a source and documentation
review, not a live browser usability audit or a new board benchmark.

## Recommendation

**Keep Angular for the household application, extend it with moderation and
announcement management, and keep a deliberately small Rust HTML layer for
bootstrap, authentication, recovery, and readable links.** We already have a
recognized browser framework and most of the proposed architecture. There is
no need to turn the Rust page helpers into an application framework.

Make browser rendering the default for new interactive features. Keep the
board responsible for authoritative data, authorization, persistence, and
bounded API responses. Do not add Angular SSR, a Node runtime, or a second
application server to the board.

REST is adequate for these workflows. Mastodon's public API does not expose
every administrative feature, but that is an API coverage issue, not a REST
limitation. Mastomini already implements report management, account moderation,
and announcement authoring endpoints. The immediate work is connecting those
capabilities to a useful UI and addressing a few integration gaps.

## What framework are we using?

| Surface | Current implementation | Where work happens |
|---|---|---|
| Household app at `/app/` | Angular; package.json specifies `^22.1.0` for core packages. Standalone components, signals, zoneless change detection, lazy routes, hash routing | Browser executes JavaScript and renders components |
| Setup, OAuth, trust, public information, post/profile/tag pages | Handwritten Rust functions with `format!`, escaping, and shared HTML/CSS helpers | Board produces HTML per request |
| Backend/API | Custom Rust request dispatch and domain services; desktop uses `tiny_http`, ESP32 uses its own bounded transport with ESP-IDF TLS | Board runs API, security, storage, and networking |
| Documentation site | MkDocs configuration in the repository | Separate documentation build; not the household app |

Evidence: [package.json](../mastomini_ui/package.json),
[Angular build configuration](../mastomini_ui/angular.json),
[app configuration](../mastomini_ui/src/app/app.config.ts),
[routes](../mastomini_ui/src/app/app.routes.ts),
[Rust dependencies](../mastomini_rs/Cargo.toml),
[HTML helpers](../mastomini_rs/src/api/html.rs), and
[transport](../mastomini_rs/src/bin/esp32/server.rs).

Angular templates are compiled during the development/build process. They are
not evaluated by the board. The board embeds precompressed assets and serves
them by exact path, with ETags, revalidation for the index, and long-lived
caching for other assets. Hash routing avoids server-side route rendering.
See [asset serving](../mastomini_rs/src/web.rs) and
[bundling](../mastomini_rs/scripts/bundle-web.mjs). Angular explicitly supports
[client-side rendering](https://angular.dev/guide/routing/rendering-strategies)
and [lazy route loading](https://angular.dev/guide/routing/loading-strategies).

There is no general Rust template engine here. Growing the handwritten HTML
layer into a comprehensive admin site would introduce a second UI architecture
alongside Angular, with duplicated forms, navigation, and state handling.

## Why server HTML still exists

The remembered bootstrap rationale is substantially correct, but “no account
yet” alone does not require server rendering: an unauthenticated JavaScript
app can provision an account. The stronger reasons are reliability in captive
portals and embedded sign-in browsers, minimal first-load dependencies, and
recovery when the main bundle cannot run.

The existing policy is recorded in [06-household-app.md](06-household-app.md).
The current implementation also includes post/profile/tag pages, so the HTML
surface is broader than initial setup alone.

| Routes/surface | Proposed treatment | Reason |
|---|---|---|
| `/setup/wifi`, first household creation (`/`, `/setup`) | Keep small server forms | Must work before home Wi-Fi and accounts are established |
| `/oauth/authorize` | Keep server form | Shared login/authorization entry point for native clients and the Angular app; avoids coupling login to the admin bundle |
| `/setup/<code>`, `/setup/password` | Keep recovery forms | Invite and password recovery links should remain independently usable |
| `/trust`, certificate download, landing/connect fallback | Keep concise server version; richer guidance in Angular | Trust installation can be needed before a client can connect |
| `/about`, `/terms-of-service`, `/privacy-policy` | Keep bounded read-only HTML | Useful without login; household-specific text cannot all be fixed at build time |
| `/@user`, `/@user/:id`, `/@user/tagged/:tag`, `/tags/:tag` | Preserve existing links and visibility checks; cap further feature growth | These are readable share links, not an admin application |
| `/web/signin`, `/web/signout` | Keep while authenticated HTML link pages exist | Their browser cookie session is separate from the Angular bearer token |
| Members, moderation, announcements, diagnostics, server settings | Angular only for new rich workflows | Browser owns composition, interaction, and presentation |

Sources: [setup](../mastomini_rs/src/api/setup.rs),
[OAuth](../mastomini_rs/src/api/oauth.rs),
[trust](../mastomini_rs/src/api/trust.rs),
[information pages](../mastomini_rs/src/api/about.rs), and
[readable pages/session handling](../mastomini_rs/src/api/pages.rs).
The readable listing pages already have a 20-entry page size.

Do not remove readable pages merely to reduce the number of HTML routes.
If measurements later implicate them, prototype browser enhancement or an
optional browser reader while retaining canonical URLs, authentication,
visibility, and no-JavaScript fallback. A full social client remains outside
the household app's current scope.

## Framework choices

| Option | Benefits | Costs and fit | Decision |
|---|---|---|---|
| Keep and extend Angular | Existing routing, forms, signals, authentication, tests, and build pipeline; suited to interactive admin workflows | Control dependency and chunk growth; build tools remain a maintenance obligation | **Recommended** |
| Evolve the handwritten Rust pages into a framework | Reuses existing helpers; minimal browser runtime | We would own routing conventions, templating safety, form handling, partial updates, and a second UI system; recurring rendering remains on the board | Keep helpers small; do not generalize them into an admin framework |
| Preact + Vite | Plausible candidate when reducing browser assets is the dominant goal; static production output | Rewrite screens, routing integration, forms, and tests; actual full-app savings must be measured | First alternative to benchmark if Angular exceeds a real resource budget |
| Vue + Vite | Recognized component framework with official SPA tooling | Comparable architecture but substantial rewrite; no demonstrated board benefit for this app | Viable team-preference alternative, no current migration case |
| Svelte / SvelteKit in static SPA mode | Recognized component approach; supports browser-only deployment | Rewrite plus careful static routing configuration; no server features available on the board | Viable alternative prototype, not an immediate migration |
| A server-oriented full-stack framework | Useful when a conventional application server owns rendering | Adds deployment/runtime concepts without solving the ESP32 constraint; a static export would still be a browser app | No board-side SSR runtime |

Official references: [Preact with Vite](https://preactjs.com/guide/v10/getting-started/),
[Vue tooling](https://vuejs.org/guide/scaling-up/tooling.html), and
[SvelteKit SPA deployment](https://svelte.dev/docs/kit/single-page-apps).
These establish deployment options, not measured superiority. No alternative
has been built or benchmarked for this report. Compare complete equivalent
screens, including routing/forms/dependencies, rather than framework headline
sizes. Switching browser frameworks does not inherently reduce API, TLS, or
persistence CPU cost.

## Rich moderation over the existing APIs

Mastodon documents REST endpoints for [administrative reports](https://docs.joinmastodon.org/methods/admin/reports/)
and [account moderation](https://docs.joinmastodon.org/methods/admin/accounts/).
They support a browser moderation console without requiring GraphQL, WebSockets,
or server-rendered forms. Rich interaction comes from UI state and workflow
design, not the transport style.

Mastomini already has:

- Report listing/detail, classification/rule updates, assign-to-self/unassign,
  resolve, and reopen under `/api/v1/admin/reports`.
- Administrative account listing/detail and actions under
  `/api/v1/admin/accounts` (also v2 account listing).
- Household member actions and administrative status deletion under
  `/api/mastomini/v1/admin/...`.
- A Members screen already exposing enable/disable, silence, and suspension.

Evidence: [moderation API](../mastomini_rs/src/api/moderation.rs),
[household API](../mastomini_rs/src/api/household.rs),
[member UI](../mastomini_ui/src/app/pages/members.ts), and
[moderation tests](../mastomini_rs/src/api/tests/moderation.rs).

Propose a lazy `/app/#/admin/moderation` area with an open/resolved report queue,
reporter/target filters, assignment indicators, and a detail panel containing
reported content, classification, rules, and available account actions.
Preserve navigation/filter state when returning from a case. Display partial
failures and deleted/evicted content explicitly. The current limits are 16
accounts and 32 reports; a heavyweight enterprise grid is unnecessary.

For destructive actions, show the target and effect, wait for server success,
and then refresh affected records. An action followed by report resolution
must show the intermediate state if only one succeeds. If atomic combined
actions become a requirement, implement one bounded domain operation on the
board rather than implying that two browser requests are transactional.

Concrete integration work required:

1. **Admin authorization scopes.** The app currently requests only `read write`.
   Report APIs require `admin:read:reports` / `admin:write:reports`, account APIs
   require their account equivalents, and announcement administration checks
   `admin:read` / `admin:write`. Ordinary `read write` does not cover these.
   Add an explicit admin authorization path and version/check stored app
   registrations so old registrations are replaced when scopes change. Merely
   adding scopes to the authorization URL is insufficient. Keep role checks on
   the server; route guards are navigation assistance only.
2. **Pagination and request control.** The current fetch wrapper returns JSON
   and discards response headers. Expose pagination links and cancellation;
   follow bounded pages instead of assuming one response contains all reports.
   Local filters must say whether they cover only loaded data.
3. **Workflow gaps.** Durable case notes, a moderation audit trail, revision
   conflicts, and bulk operations should be treated as new requirements, not
   assumed existing features. Start with single-case actions; specify retention
   and capacity before adding persistent history.
4. **Retries.** Spec 06 says every household mutation takes an Idempotency-Key,
   but the reviewed wrapper does not send one and the implementation found is
   for status creation. Do not assume general mutation deduplication. On an
   ambiguous timeout, re-read state before retrying; design bounded server-side
   deduplication for create/bulk workflows that need safe replay.

Sources: [auth service](../mastomini_ui/src/app/api/auth.ts),
[fetch wrapper](../mastomini_ui/src/app/api/api.ts),
[scope semantics](../mastomini_rs/src/domain/oauth.rs), and
[status idempotency](../mastomini_rs/src/domain/statuses.rs).

Keep the current privacy boundary: reported DMs do not become visible to an
unrelated admin merely because this is a moderation screen. Preserve the
server's filtering, including the behavior covered by
[DM API tests](../mastomini_rs/src/api/tests/dm.rs).

## Announcement management

Add `/app/#/admin/announcements` with list, editor, preview, draft/publish,
unpublish, delete, and visibility-window controls. This is primarily UI work:
[social API](../mastomini_rs/src/api/social.rs) already supports:

| Operation | Implemented endpoint |
|---|---|
| List/create | `GET` / `POST /api/v1/admin/announcements` |
| Read/update/delete | `GET` / `PUT` / `PATCH` / `DELETE /api/v1/admin/announcements/:id` |
| Publish/unpublish | `POST /api/v1/admin/announcements/:id/publish` or `/unpublish` |
| Member consumption | `/api/v1/announcements`, dismissal, and reactions |

Treat the authoring routes as Mastomini extensions: the upstream
[announcement API documentation](https://docs.joinmastodon.org/methods/announcements/)
documents consumption, dismissal, and reactions, not this authoring contract.
Do not promise that an arbitrary Mastodon client can administer announcements.

The domain currently allows 16 announcements with at most 2,048 UTF-8 bytes of
text each. The editor should count bytes, use plain text with a safe preview,
show capacity, and explicitly send `published: false` when creating a draft:
the current create default is published. See
[announcement domain behavior](../mastomini_rs/src/domain/social.rs).

Convert browser-local date input to RFC3339 UTC values, showing the timezone
and handling invalid local times. Explain that visibility currently requires
`published` and that board time falls inside the optional start/end window.
This is evaluated on reads; it does not require a browser tab to remain open.
Show a clock warning if board time is unset. The `all_day` flag does not replace
the current timestamp visibility checks. A blank timestamp clears it in the
existing API; distinguish clear from leave-unchanged in the editor.

Refresh after mutations and preserve edits on errors. Before supporting
multiple simultaneous editors, add a revision/precondition contract to avoid
silent overwrites. Do not implement automatic repeated creation after a lost
response until duplicate handling is defined.

## Move presentation work to the browser, retain authority on the board

| Browser responsibilities | Board responsibilities |
|---|---|
| Component rendering, navigation, dialogs, and responsive layout | Authenticate and enforce role, scope, and resource visibility on every request |
| Sorting/filtering already-authorized, bounded datasets | Apply authoritative filters/pagination and never return hidden data for client-side filtering |
| Form feedback, byte counts, dates/timezones, previews | Repeat validation, enforce limits, and decide announcement visibility using board time |
| Charts and summaries of bounded diagnostic samples | Maintain bounded counters and diagnostic records |
| QR generation; prospective image resize/compression before upload | Validate uploaded content, size, and persistence rules |
| Transient workflow state and explicit refresh | Commit mutations, moderation effects, audit records if added, and credential revocation |

Keep the Mastodon API's required content representation for third-party clients.
Moving Angular presentation to the browser does not justify changing standard
API HTML fields into a private raw-text protocol. Add targeted household
responses only when a measured workflow needs them.

Use plain Angular signals/services and small reusable UI components for shared
dialogs, errors, empty states, and action feedback. Add workers only if a real
browser task causes noticeable blocking; a 32-report queue does not justify
one. Do not move authoritative password verification, moderation decisions, or
storage enforcement into JavaScript.

Avoid increasing board work through an overly chatty SPA:

- Lazy-load admin routes; do not preload every feature after sign-in.
- Deduplicate reads and limit simultaneous API requests initially to one or two
  per tab; tune with measurements. Avoid a request per row.
- Start with refresh-on-entry and after mutations. If polling is needed, use a
  slow configurable interval, pause hidden tabs, and back off on failure.
- Keep viewer-specific data in bounded memory and clear it on sign-out or role
  changes. Do not persist moderation content in a generic browser cache.
- Consider one small aggregate read endpoint only if measured request fan-out
  warrants it. Keep it authorized and bounded; GraphQL is not a prerequisite.
- Retain static asset caching and same-origin serving. Audit the bundler's
  assumption that every non-index asset is immutable before adding unversioned
  configuration or public files.

Do not make a CDN, service worker, or secure-context-only browser capability a
bootstrap dependency. Existing plain-HTTP/LAN and certificate-installation
paths must still work. Continue safe text rendering and Angular sanitization;
never bypass sanitization for reported user content. Admin bearer tokens are
currently stored in localStorage, so extending their privileges makes XSS
prevention particularly consequential.

## Resource evidence and what still needs measuring

The existing `.embuild/web/files` directory contains 16 gzip files totaling
124,203 bytes (about 121.3 KiB). This is an inspected local artifact, **not a
fresh build or proof that it matches every current source change**. Spec 06's
104 KiB figure describes an earlier build.

Three budgets need distinct names and checks:

- Angular currently warns at 400 kB and fails at 600 kB for its **initial build
  bundle**. That is not the total gzip asset footprint across lazy routes.
- Spec 06 proposes a **600 KiB embedded UI budget**. The bundler currently logs
  total gzip size but does not enforce that ceiling. A future implementation
  should enforce it, including all lazy assets.
- The [partition table](../mastomini_rs/partitions.csv) gives the whole firmware
  application 4 MiB. Embedded assets share that with Rust and TLS; satisfying
  the UI budget alone does not prove the firmware fits with useful headroom.

Precompression saves runtime compression work, but serving static assets still
costs TLS, transfer, and response memory. `web.rs` currently copies each gzip
asset into a response Vec. Consequently, largest individual chunks and parallel
cold loads matter as well as total flash footprint. A future borrowed/static
response body is worth considering only if those copies materially affect
heap pressure.

[Existing performance investigations](08-performance.md) identify transport,
connection setup, and request scheduling costs. The recorded transport fix
reduced established-request interference during a silent handshake from about
4,954 ms to a 95 ms mean / 109 ms maximum in that probe. These are historical
measurements, not a result of this review. They do not establish HTML rendering
as the bottleneck, and the public metadata load tests do not establish rich
moderation capacity.

Before replacing frameworks or removing HTML, measure on the same firmware,
dataset, Wi-Fi conditions, and browser:

1. Cold and warm app load: compressed bytes, request count, TLS connections,
   time to usable controls, peak response memory, and heap low-water mark.
2. Open report queue/detail, execute an action, and publish an announcement:
   request count, API/lock time, p50/p95 completion, and failure rate.
3. One and several admin/member clients together, including interrupted Wi-Fi,
   bounded full datasets, and expired credentials.
4. Representative HTML pages versus their API/rendering work, separating
   handler CPU, JSON/HTML serialization, TLS, and transfer time.

Keep Angular unless an equivalent alternative demonstrably meets an otherwise
unattainable flash/heap/load-time budget, or the team explicitly prefers a
different framework enough to fund a rewrite. Bundle size alone is not a
measurement of server CPU saved.

## Proposed implementation sequence

1. **Establish baselines and contracts.** Fresh production bundle/firmware size
   report; workflow timing; document scope migration, paging, error/retry, and
   announcement extension contracts. Reconcile the idempotency statement in
   spec 06 with implemented behavior.
2. **Deliver announcements as a vertical slice.** Lazy route, admin authorization,
   draft/edit/publish lifecycle, safe preview, timestamps, capacity feedback,
   and focused UI/API integration tests. Re-measure cold/warm loading.
3. **Deliver moderation.** Queue/detail, assignment and classification, existing
   member actions, resolution/reopen, explicit destructive confirmations, and
   partial-failure handling. Preserve server privacy and role restrictions.
4. **Add deeper workflow support only as required.** Revision checks, bounded
   audit history/notes, and safe bulk operations each need an explicit domain
   and storage contract. They are not framework features.
5. **Revisit HTML and framework choice from evidence.** Freeze expansion of the
   Rust HTML surface meanwhile. Optimize measured hot paths before a rewrite.

Acceptance checks for that future work: native-client OAuth and captive setup
still work; non-admin API calls fail even with manually crafted requests;
missing scopes trigger a useful authorization flow; old registrations migrate;
pagination has no hidden omissions; stale/deleted reports remain understandable;
announcements survive restart and obey board-time windows; connection failures
do not silently duplicate mutations; reported HTML cannot execute scripts;
keyboard/mobile workflows work; and gzip/firmware/heap budgets are recorded.

This report authorizes no implementation or deployment. Its proposed direction
is to grow the browser application we already have, while making the server
HTML boundary explicit and keeping board costs measurable.

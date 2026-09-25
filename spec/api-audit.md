# REST audit and maintenance

Audited 2026-09-25 against the **working tree**, including the in-progress HTTPS
changes. This is the implementation-status authority; older “done” labels in
the sprint plan describe deliveries, not complete REST parity.

- [Endpoint inventory](api-coverage.md): every method/path in the reference
  snapshot plus additional local REST routes, with a classification and source.
- [Machine-readable ledger](api-coverage.json): edit this, then regenerate the table.
- [Performance investigation](08-performance.md): findings, measurements to collect,
  and an ordered tuning plan.

Reference: `mimb/mastodon_mock` commit
`e09c035725b5dfdbc8bf2b8efce633d06cc35fed` (2026-09-19), its
`spec/03-api-coverage.md`, pinned `mastodon-openapi/dist/schema.json` (4.7.0),
and the actual mastomini routers/domain code. The schema hash is in the ledger.
The reference includes versions beyond mastomini's advertised 4.3; missing a
newer endpoint does not necessarily break a 4.3 client. The mock's own REST
checklist is not a perfect OpenAPI inventory, so both sources are used.
Mock control endpoints, household APIs, static files and HTML pages are outside
this REST table. Discovery and OAuth endpoints are included.

## Delivered local features (2026-09-25)

| Feature | Implemented behavior / explicit limit |
|---|---|
| Followed and featured tags | Persistent, normalized, idempotent; 32 followed and 10 featured per member. Followed public tags enter home; suggestions, usage history and profile counts respect visibility. |
| Private account notes | Persisted per viewer/target, 2000 UTF-8 bytes, empty comment clears; private to that viewer. |
| Suggestion dismissal (backlog 3) | Persisted and filtered in both suggestion API versions. |
| Grouped notifications (backlog 5) | Favourite/reblog groups by target, follow group, individual other types; pagination, unread counts, accounts and group dismissal. Groups use the existing bounded RAM ring; no historical/time-window grouping. |
| Announcements | Admin CRUD, publish/unpublish, publication windows; member listing, dismissal and Unicode reactions. REST management is documented in the administration guide. |
| Compatibility aliases | Profile GET/PATCH, avatar/header DELETE (generated defaults), OAuth userinfo, legacy search and direct timeline. Uploads remain unsupported. |

All new persisted account references use public account IDs and are cleaned on
account deletion/reboot; slot reuse cannot inherit another member's preferences.
Rust tests cover ownership, bounds, scopes, visibility, reboot and injected write
failures. Applicable upstream tests now run normally instead of strict xfails.

## Next local backlog

**Notification policy/request handling (backlog 4) remains deferred.** This is
larger than persisting a few settings: accept/drop/filter decisions must run at
notification generation, filtered notifications need a bounded pending queue,
and request acceptance/dismissal/counts must agree with both API versions and
moderation. GET/PUT still return accept-all; PUT is a no-op and PATCH is missing.
The ledger deliberately retains these as stubs/missing, with strict xfails for
policy effects. Implement storage, generation and request actions together.

Tier 1 profile updates are **partial**: text/source fields work, uploaded
avatars/headers return 422. Status history keeps only three prior versions;
old poll snapshots are not preserved. These limits are explicit in the ledger.
Lists, filters, conversations, edits, polls, moderation, admin accounts/reports,
and collections have substantive implementations. This audit does not certify
every optional parameter or scope on those endpoints; contract tests remain
the behavioral check.

## Deliberate omissions are not missing local features

Federation-related domain controls and peers have no purpose here. Public
registration, email/IP sign-up administration and external fetching also do not
fit the household design. Media, push, scheduling and streaming are separate
hardware/product decisions. Streaming especially needs a socket budget.

Trends, endorsements, translation, preview cards, quotes and annual reports are
currently excluded by spec/04. Some *could* work locally (notably endorsements
and quotes); “no federation” is not a technical reason to forbid them. Revisit
those product decisions after the local Tier 2 backlog, instead of counting
their empty responses as completed features. Featured and followed tags are implemented local features.

## Keep this accurate without another whole-tree review

From `mastomini_rs/`:

```sh
make api-coverage
# Optional: detect reference additions/removals without running its server:
uv run --project clienttests python scripts/api-coverage.py --reference ../../mimb/mastodon_mock
```

`make check` includes the first command. The check validates classifications,
unique method/path pairs, reference coverage, evidence files, generated output,
and hashes of API/domain/shared implementation files (including record kinds and namespaces). New/deleted files in
the monitored API/domain directories are also detected. Line endings are
normalized. This deliberately flags semantic edits even when the route itself
does not change.

For a change, review only the flagged files and their affected endpoints:

1. Edit `api-coverage.json`: add/remove routes, update status, tier, explanation
   and source. “Implemented” requires meaningful behavior; an empty/no-op
   handler is a stub. Keep deliberately unsupported behavior separate.
2. Add or update behavioral tests. Remove corresponding strict xfails when a
   feature works; do not replace failures with skips to make the gate green.
3. Record the reviewed file(s), using repository-relative paths:

   ```sh
   uv run --project clienttests python scripts/api-coverage.py --acknowledge mastomini_rs/src/api/misc.rs --write
   ```

4. Run `make api-coverage` and relevant Rust/client/conformance checks.

On reference refresh, compare using `--reference`, classify new entries, update
`reference_endpoints` and provenance, then regenerate. No other checkout is
required for the normal gate. The tool never imports the mock Python server.

**Limit:** fingerprints force review; they do not prove that the reviewer updated
every row correctly. This is not a Rust route parser, live endpoint probe, or
automatic parity certification. Keep routing precedence in mind (account list
routes, for example, are dispatched before the generic account router).
Use the tests in `api/tests`, `clienttests`, and the verbatim `conformance/tests`
for behavioral evidence; exceptions live in `conformance/deviations.py`.

# 07 — Testing

## Layers

| Layer | Runs on | What it proves |
|---|---|---|
| Unit tests (`cargo test`) | desktop | domain rules, character counting table, HTML rendering, ID monotonicity, limits and error codes |
| Store tests | desktop, `MemStore` / `FaultStore` / `FileStore` | power cut after every Nth store operation during post/delete/evict/edit/password-change/account-delete leaves a store that boots, passes invariants, and loses at most the in-flight action |
| Allocation tests | desktop | zero heap growth after startup across 5,000 mixed requests at full capacity (nanacoin's `tests/allocation.rs` pattern) |
| Capacity tests | desktop | fill every limit in 03, then prove eviction order, pinned exemption, orphan cleanup, and that RAM/flash stay inside budget (NVS entry usage estimated by a model of NVS's 32-byte entry layout) |
| API conformance | desktop HTTP server | Mastodon.py-based suite, see below |
| Schema conformance | desktop | every response validates against the pinned `mastodon-openapi` schema (reuse `mastodon_mock/openapi_compare.py`) |
| Firmware build gates | desktop | size gate for `factory`, partition table check, certs-check (nanacoin scripts) |
| Board probe | board | `probe-board.py`: TLS chain, hostname, `/api/v1/instance`, OAuth round-trip with a test account, household app shell |
| Soak | board | 24 h scripted client load (4 concurrent clients, polling like Ivory/Tusky), heap high-water marks, write-governor counters, NVS stats before/after |
| Power-cut | board (manual/relay) | 200 random power cuts during scripted writes, then boot + invariant check each time |
| Client matrix | real devices | see below |

## Differential testing against mastodon_mock

`make conformance` runs mastodon_mock's own Mastodon.py contract suite against
the mastomini desktop binary (`mastomini_rs/conformance/`).

- **The tests are copied verbatim** by `make conformance-sync`
  (`sync_upstream.py`, source commit in `UPSTREAM.txt`), 25 files that talk to a
  server only over HTTP. Tests that drive mock internals (`MockServer` seeds,
  fault injection, CLI, UI, alembic, unit tests) are not copied. Nothing in
  mastodon_mock changes for mastomini's sake, and `tests/` is never edited by
  hand.
- **`conftest.py` provides the mock's fixtures** (`live_server`, `alice`, `bob`,
  `carol`, `mastodon_client`, …) on the real binary. The seed (alice owns the
  household, bob and carol are members, carol is locked, alice follows bob) is
  created once per session through `/api/mastomini/v1` and the genuine OAuth
  form flow. Each test then starts its own server on a byte-for-byte copy of that
  store file. The tests' literal `"alice_token"`-style bearer tokens are mapped to
  the real tokens at the client boundary (Mastodon.py, httpx2, requests). The
  server has no test shortcut.
- **`deviations.py` is the list of known differences**, as data:
  `NOT_APPLICABLE` (skipped: federation, media, sign-ups, mock helpers, trends, …),
  `NOT_YET` (strict xfail: lists, filters, edits, conversations, follow requests,
  followed tags, …) and `DIFFERENT` (strict xfail: 140 characters, no grouping,
  and places where mastomini follows real Mastodon and the mock does not).
  Strict xfail means a test that starts passing fails the run until its entry is
  removed. Anything failing that isn't listed is a bug.

First result (2026-09-23): 87 of 210 passed as-is; after fixes, 96 pass, 69 are
not applicable and 45 are listed deviations, with no unlisted failures.

Python tooling runs under `uv run` (per workspace convention). Rust through
`cargo` via the Makefile in Git Bash.

## Client compatibility matrix (Phase 5)

For each client, record: connects, signs in, home timeline, post, reply,
favourite/boost, notifications, profile edit, what breaks. Also: does it tolerate
`max_media_attachments: 0`, does it need streaming, does it accept the household CA.

| Client | Platform | Cert model to test |
|---|---|---|
| Ivory | iOS/macOS | A, B |
| Ice Cubes | iOS/macOS | A, B |
| Mona | iOS/macOS | A |
| Mastodon (official) | iOS, Android | A (iOS), B (Android) |
| Tusky | Android | B |
| Moshidon | Android | B |
| Phanpy | browser | A (trusted), B |
| Elk | browser | B (probably can't: server-side parts) |
| Whalebird, Tuba | desktop | A |
| Household app | any browser | A, B |

The matrix feeds the `/connect` guide's data file.

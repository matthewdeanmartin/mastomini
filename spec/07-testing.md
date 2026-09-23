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

`mimb/mastodon_mock` already runs a Mastodon.py-based suite against itself and ships
pytest fixtures. The plan:

1. Add a `mastomini` target to a copy of the relevant integration tests: start the
   mastomini desktop binary, seed accounts through `/api/mastomini/v1`, and log in
   through the real OAuth code flow (scripted form post). mastomini offers no
   `/_mock/login` shortcut.
2. Run the **same client scripts** against both servers and diff the responses
   after normalizing IDs, timestamps and hostnames. Differences are either bugs or
   documented deviations (a list checked into `spec/deviations.md` when it exists).
3. Exclude what mastomini deliberately lacks: media, federation, admin API, mock
   helpers.

This gives a quick answer to "does this behave like Mastodon?" without a real
Mastodon instance.

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

# Contributor guide

## Layout

| Path | What |
|---|---|
| `mastomini_rs/src/domain/` | Household state and rules: accounts, posts, follows, moderation, collections, encrypted direct messages. No I/O except through the `Store` trait |
| `mastomini_rs/src/store/` | The key/value store with NVS semantics (`MemStore`, `FileStore` for the desktop, `NvsStore` on the board) |
| `mastomini_rs/src/api/` | The Mastodon REST API, the household API (`/api/mastomini/v1`) and the small server-rendered pages, over transport-neutral request/response types |
| `mastomini_rs/src/crypto.rs` | Direct-message encryption primitives |
| `mastomini_rs/src/bin/desktop.rs` | The desktop server |
| `mastomini_rs/src/bin/esp32*` | The firmware: Wi-Fi, onboarding, mDNS, HTTP, NVS |
| `mastomini_rs/clienttests/` | Mastodon.py tests against the real desktop binary |
| `mastomini_rs/conformance/` | mastodon_mock's contract suite, copied verbatim, with known differences in `deviations.py` |
| `spec/` | Design documents and decisions. Read `spec/README.md` first |
| `docs/` | This site |

## Quality gates

Everything runs from Git Bash in `mastomini_rs/`:

```bash
make check
```

That runs `cargo fmt --check`, `cargo clippy -- -D warnings`, the Python
tooling check, `cargo test`, the HTTP smoke test, the Mastodon.py client suite
and the conformance suite. A change is done when `make check` passes, and, for
anything that touches the firmware, `make firmware` still builds.

Python tools always run through `uv run`.

## REST coverage and performance

Start with [the REST audit](https://github.com/matthewdeanmartin/mastomini/blob/main/spec/api-audit.md)
and [endpoint inventory](https://github.com/matthewdeanmartin/mastomini/blob/main/spec/api-coverage.md)
when choosing API work. They distinguish implemented behavior, partial features,
empty/no-op stubs, missing endpoints and deliberate exclusions. Sprint completion
does not mean every endpoint in a feature family is implemented.

`make api-coverage` (also part of `make check`) detects changes to reviewed
implementation files and a stale generated table. Update the JSON ledger,
review the affected endpoints, and acknowledge only the files you reviewed;
the audit documents the commands. Behavioral conformance still needs tests.

For latency work, see [the performance investigation](https://github.com/matthewdeanmartin/mastomini/blob/main/spec/08-performance.md)
and `mastomini_rs/scripts/bench-api.py`. Compare new and reused connections and
burst concurrency before adding caches: normal social API reads already use RAM.

## Principles

- **Same wire shapes as Mastodon.** Every field Mastodon defines is present,
  with a neutral value where mastomini lacks the feature.
- **Every user action has one commit point** in the store. Anything a power cut
  can interrupt is finished or rolled back at boot. Tests cut power after every
  write to prove it.
- **Every collection is bounded** (`spec/03-limits.md`).
- **Nothing on the read path writes to flash.** Notifications and read markers
  live in RAM only.

## Conformance with mastodon_mock

`make conformance` runs mastodon_mock's own Mastodon.py contract tests against
the desktop binary, with real sign-ins. The tests are copied, never edited;
`make conformance-sync` refreshes them. When a test fails, either fix
mastomini or add the test to `conformance/deviations.py` with a reason. Those
entries are strict: a listed test that starts passing fails the run until its
entry is removed.

## Building these docs

```bash
uvx --with-requirements docs/requirements.txt mkdocs serve   # from the repository root
```

Read the Docs builds the site from `.readthedocs.yaml` and `mkdocs.yml`.

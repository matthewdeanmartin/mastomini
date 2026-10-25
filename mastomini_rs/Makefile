SHELL := bash
.DEFAULT_GOAL := help

CARGO ?= cargo
UV ?= uv
ESP_PYTHON ?= /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe

.PHONY: help run run-web web web-check ui-test build test lint fmt fmt-check scripts-check smoke client-test conformance conformance-sync docs docs-serve check certs certs-check reissue-cert rotate-certs firmware deploy install boot-log probe-board probe-mdns board-version api-coverage move-export move-import firmware-takeover deploy-takeover

help:
	@printf '%s\n' \
	  'make run          Desktop server at http://127.0.0.1:8080 (and https://localhost:8443 after make certs)' \
	  'make build        Build the desktop server' \
	  'make test         Rust unit and integration tests' \
	  'make lint         Clippy with warnings as errors' \
	  'make fmt          Format Rust source' \
	  'make smoke        OAuth + posting smoke test against the real binary' \
	  'make web          Build the household app (../mastomini_ui) for embedding' \
	  'make run-web      Desktop server with the household app at /app/' \
	  'make ui-test      Household app unit tests (vitest)' \
	  'make web-check    Rust tests with the household app embedded' \
	  'make client-test  Mastodon.py suite against the real binary (uv)' \
	  'make conformance  mastodon_mock contract suite against the real binary' \
	  'make conformance-sync  Re-copy the contract tests from ../../mimb/mastodon_mock' \
	  'make api-coverage Validate REST endpoint ledger and detect source drift' \
	  'make bench-read   Release CPU benchmark at 4096 posts' \
	  'make bench-desktop Disposable authenticated HTTP burst benchmark' \
	  'make bench-transport ADDRESS=ip LABEL=name  Read-only board benchmark: HTTP/1.1 vs HTTP/2 (loadtests/)' 	  'make diag-h2-burst ADDRESS=ip   Browser-style HTTP/2 burst: every stream answered with CORS?' 	  'make diag-transfer ADDRESS=ip   Large-response transfer times and mid-transfer stalls' 	  'make diag-browser ADDRESS=ip    Real Firefox/Chromium as a cross-origin client (BROWSER=chromium)' 	  'make diag-firefox-log LOG=file  Why Firefox closed the board connections (about:logging capture)' 	  'make diag-har HAR=file          HAR timeline for the board, tokens redacted' \
	  'make docs         Build the docs site strictly (mkdocs, as Read the Docs does)' \
	  'make docs-serve   Preview the docs at http://127.0.0.1:8000' \
	  'make check        All quality gates' \
	  'make certs        Household CA + HTTPS certificate (creates only what is missing)' \
	  'make certs-check  Check the certificate files without a board' \
	  'make reissue-cert New HTTPS certificate from the same CA (renewal, new name or IP)' \
	  'make rotate-certs New household CA: every device must trust it again' \
	  'make firmware                   Build the ESP32-S3 images (no board access)' \
	  'make deploy PORT=COM11          Upgrade the app on a mastomini board (DEPLOY.md)' \
	  'make install PORT=COM11 MAC=..  FIRST INSTALL ONLY: backup, erase, flash (DEPLOY.md)' \
	  'make boot-log PORT=COM11        Capture the boot log and the board address' \
	  'make probe-board ADDRESS=ip     Strict read-only check of a running board' \
	  'make probe-mdns ADDRESS=ip      Compare IP access, mDNS replies and OS name lookup' \
	  'make board-version ADDRESS=ip   Does the board run this working tree? (exit 1: flash it)' \
	  'make firmware-takeover BOARD=p4 Build BOARD under the household identity (mastomini, certs/)' \
	  'make deploy-takeover PORT=COM19 Upgrade the household P4 (the household board since 2026-10-21)' \
	  'make move-export ADDRESS=ip     Download the household from a board (.local/move/, owner from .env)' \
	  'make move-import ADDRESS=ip FILE=..  Restore it onto an erased board (spec/board_migration_plan.md)'

# HTTPS next to HTTP once make certs has run (docs/security/https.md).
HTTPS_PORT ?= 8443

run:
	MASTOMINI_HTTPS_PORT=$(HTTPS_PORT) $(CARGO) run --locked

build:
	$(CARGO) build --locked

test:
	$(CARGO) test --locked

lint:
	$(CARGO) clippy --locked --all-targets -- -D warnings

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt --check

smoke: build
	$(UV) run --project clienttests python clienttests/smoke.py

client-test: build
	$(UV) run --project clienttests pytest clienttests -q

# mastodon_mock's tests, copied verbatim; known differences in
# conformance/deviations.py (spec/07 "Differential testing").
conformance: build
	$(UV) run --project conformance pytest conformance -q

# The docs live at the repository root (mkdocs.yml, docs/), built by Read the Docs.
docs:
	cd .. && $(UV)x --with-requirements docs/requirements.txt mkdocs build --strict

docs-serve:
	cd .. && $(UV)x --with-requirements docs/requirements.txt mkdocs serve

conformance-sync:
	$(UV) run --project conformance python conformance/sync_upstream.py

# Byte-compiles the Python tooling (deploy, probe, client tests) and parses
# the shell scripts: catches syntax errors in scripts that only run against
# real hardware.
scripts-check:
	$(UV) run --project clienttests python -m py_compile scripts/board.py scripts/boot-log.py \
	  scripts/deploy.py scripts/firmware-image.py scripts/install.py scripts/probe-board.py scripts/firmware-version.py \
	  clienttests/conftest.py clienttests/harness.py clienttests/smoke.py clienttests/test_mastodon_py.py \
	  clienttests/test_https.py clienttests/test_bots_and_pages.py \
	  clienttests/test_audit_tools.py scripts/api-coverage.py scripts/bench-api.py scripts/bench-desktop.py \
	  scripts/probe-connection-queue.py scripts/probe-transport.py scripts/probe-mdns.py \
	  loadtests/locustfile.py loadtests/sweep.py loadtests/test_readonly.py \
	  conformance/conftest.py conformance/deviations.py conformance/sync_upstream.py
	bash -n scripts/certs.sh
	bash -n scripts/certs-check.sh
	bash -n scripts/rotate-certs.sh

api-coverage:
	$(UV) run --project clienttests python scripts/api-coverage.py

.PHONY: perf-quick
# ~5 minutes: handshakes, HTTP/2 burst, short transport and sweep, transfers.
perf-quick:
	bash loadtests/quick_perf.sh "$(ADDRESS)" "$(or $(HOST),mastomini.local)" "$(or $(CA),certs/household-ca.crt)" $(LABEL)

.PHONY: diag-h2-burst diag-transfer diag-browser diag-firefox-log diag-har
# Connection diagnostics (loadtests/; README "Connection diagnostics"). Read
# requests only; a token is optional (loadtests/oauth_token.py).
diag-h2-burst:
	$(UV) run --project loadtests python loadtests/h2_burst.py --address "$(ADDRESS)" $(ARGS)

diag-transfer:
	$(UV) run --project loadtests python loadtests/transfer_stalls.py --address "$(ADDRESS)" --scheme both $(ARGS)

diag-browser:
	$(UV) run --project loadtests python loadtests/browser_session.py --address "$(ADDRESS)" --browser "$(or $(BROWSER),firefox)" $(ARGS)

diag-firefox-log:
	$(UV) run --project loadtests python loadtests/firefox_log.py "$(LOG)" --timeline $(ARGS)

diag-har:
	$(UV) run --project loadtests python loadtests/har_summary.py "$(HAR)" $(ARGS)

.PHONY: bench-read bench-desktop bench-transport
# Anonymous GETs only (loadtests/transport.py); keep the reports you compare.
bench-transport:
	$(UV) run --project loadtests python loadtests/transport.py --address "$(ADDRESS)" --label "$(LABEL)" $(ARGS)

bench-read:
	$(CARGO) run --release --locked --example read_bench -- 4096 500

bench-desktop:
	$(CARGO) build --release --locked
	$(UV) run --project clienttests python scripts/bench-desktop.py --output ../.local/bench-desktop.json

check: api-coverage fmt-check lint scripts-check docs test ui-test web-check smoke client-test conformance

web:
	bash scripts/build-web.sh

run-web: web
	MASTOMINI_HTTPS_PORT=$(HTTPS_PORT) $(CARGO) run --locked --features bundled-web

ui-test:
	cd ../mastomini_ui && npm test

web-check: web
	$(CARGO) test --locked --features bundled-web

# HTTPS certificates (docs/security/https.md). The CA's private key stays in
# .local/ca (or MASTOMINI_CA_DIR); only certs/ goes into the firmware.
certs:
	bash scripts/certs.sh

certs-check:
	bash scripts/certs-check.sh

reissue-cert:
	bash scripts/certs.sh --reissue

rotate-certs:
	bash scripts/rotate-certs.sh --yes

firmware:
	bash scripts/build-esp32.sh

deploy:
	bash scripts/deploy.sh "$(PORT)"

install:
	bash scripts/install.sh "$(PORT)" --confirm-mac "$(MAC)"

boot-log:
	"$(ESP_PYTHON)" scripts/boot-log.py --port "$(PORT)"

probe-board:
	$(UV) run --project clienttests python scripts/probe-board.py --address "$(ADDRESS)"

board-version:
	$(UV) run --project clienttests python scripts/firmware-version.py --address "$(ADDRESS)"

# Moving the household to another board (DEPLOY.md "Moving the household").
# The new board takes the household's name and certificate, so signed-in
# devices and bots follow it; only once the old board is off.
#
# Since 2026-10-21 the household board is the P4: upgrade it with
# `make deploy-takeover PORT=COM19` (BOARD defaults to p4), not `make deploy`.
BOARD ?= p4
TAKEOVER := MASTOMINI_BOARD=$(BOARD) MASTOMINI_HOSTNAME=mastomini MASTOMINI_CERT_DIR=certs HTTP2=1

firmware-takeover:
	$(TAKEOVER) bash scripts/build-esp32.sh

deploy-takeover:
	$(TAKEOVER) bash scripts/deploy.sh "$(PORT)"

move-export:
	$(UV) run --project loadtests python loadtests/household_move.py export --address "$(ADDRESS)"

move-import:
	$(UV) run --project loadtests python loadtests/household_move.py import --address "$(ADDRESS)" --file "$(FILE)"

probe-mdns:
	$(UV) run --project clienttests python -u scripts/probe-mdns.py --address "$(ADDRESS)"

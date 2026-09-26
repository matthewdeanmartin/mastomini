SHELL := bash
.DEFAULT_GOAL := help

CARGO ?= cargo
UV ?= uv
ESP_PYTHON ?= /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe

.PHONY: help run run-web web web-check ui-test build test lint fmt fmt-check scripts-check smoke client-test conformance conformance-sync docs docs-serve check certs certs-check reissue-cert rotate-certs firmware deploy install boot-log probe-board board-version api-coverage

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
	  'make board-version ADDRESS=ip   Does the board run this working tree? (exit 1: flash it)'

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
	  conformance/conftest.py conformance/deviations.py conformance/sync_upstream.py
	bash -n scripts/certs.sh
	bash -n scripts/certs-check.sh
	bash -n scripts/rotate-certs.sh

api-coverage:
	$(UV) run --project clienttests python scripts/api-coverage.py

.PHONY: bench-read bench-desktop
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

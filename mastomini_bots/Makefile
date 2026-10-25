SHELL := bash
.DEFAULT_GOAL := help

CARGO ?= cargo
UV ?= uv
# mastomini's client-test environment (requests, Mastodon.py, pytest).
CLIENTTESTS ?= ../mastomini_rs/clienttests

.PHONY: help run run-web build test lint fmt fmt-check web ui-test e2e e2e-llm check scripts-check setup-nanacoin coverage certs reissue-cert firmware deploy install boot-log probe-board

help:
	@printf '%s\n' \
	  'make run          Desktop server at http://127.0.0.1:8090 (HTTPS :8453 after make certs)' \
	  'make run-web      The same, with the admin app at /app/ (builds it first)' \
	  'make test         Rust unit tests (bots, schedules, time zones, scheduler, admin API)' \
	  'make lint         Clippy with warnings as errors' \
	  'make fmt          Format Rust source' \
	  'make web          Build the admin app (../mastomini_bots_ui) for embedding' \
	  'make ui-test      Admin app unit tests (vitest)' \
	  'make setup-nanacoin ARGS="--url https://mastomini-bots.local"  Create and connect the NanaCoin bots' \
	  'make coverage     Rust library unit-test coverage (requires cargo-llvm-cov and llvm-tools)' \
	  'make e2e          A real mastomini + mastomini-bots: good_morning posts through the admin API' \
	  'make e2e-llm OPENROUTER_ENV_FILE=path/.env   The LLM bots with a real model (a few cents at most)' \
	  'make check        All of the above gates' \
	  'make certs        Admin site certificate from the household CA (creates only what is missing)' \
	  'make reissue-cert New certificate, same CA (new name or IP: MASTOMINI_BOTS_CERT_IPS=...)' \
	  'make firmware     Build the ESP32-S3 images (no board access)' \
	  'make deploy PORT=COM12          Upgrade the app on a mastomini-bots board (DEPLOY.md)' \
	  'make install PORT=COM12 MAC=..  FIRST INSTALL ONLY: backup, erase, flash (DEPLOY.md)' \
	  'make boot-log PORT=COM12        Capture the boot log and the board address' \
	  'make probe-board ADDRESS=ip     Strict read-only check of a running bots board'

HTTPS_PORT ?= 8453

run:
	MASTOBOTS_HTTPS_PORT=$(HTTPS_PORT) $(CARGO) run --locked

run-web: web
	MASTOBOTS_HTTPS_PORT=$(HTTPS_PORT) $(CARGO) run --locked --features bundled-web

build:
	$(CARGO) build --locked

test:
	$(CARGO) test --locked --features e2e

lint:
	$(CARGO) clippy --locked --all-targets --features e2e -- -D warnings

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt --check

web:
	bash scripts/build-web.sh

ui-test:
	cd ../mastomini_bots_ui && npm test

e2e: build
	$(CARGO) build --locked --manifest-path ../mastomini_rs/Cargo.toml
	$(UV) run --project $(CLIENTTESTS) pytest e2e -q -p no:cacheprovider

e2e-llm: build
	$(CARGO) build --locked --manifest-path ../mastomini_rs/Cargo.toml
	bash scripts/e2e-llm.sh

check: fmt-check lint scripts-check test ui-test e2e

setup-nanacoin:
	$(UV) run --no-build scripts/setup-nanacoin.py $(ARGS)

coverage:
	$(CARGO) llvm-cov --locked --lib --features e2e --html --output-dir .local/coverage

# Byte-compiles the board tooling and parses the shell scripts: catches
# syntax errors in scripts that only run against real hardware.
scripts-check:
	$(UV) run --project $(CLIENTTESTS) python -m py_compile scripts/board.py scripts/install.py \
	  scripts/deploy.py scripts/boot-log.py scripts/probe-board.py scripts/firmware-image.py scripts/setup-nanacoin.py
	$(UV) run --no-build scripts/setup-nanacoin.py --help >/dev/null
	$(UV) run --no-build --with httpx2==2.0.0 python scripts/test_setup_nanacoin.py
	for f in scripts/*.sh; do bash -n "$$f"; done

certs:
	bash scripts/certs.sh

reissue-cert:
	bash scripts/certs.sh --reissue

firmware:
	bash scripts/build-esp32.sh

ESP_PYTHON ?= /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe

deploy:
	bash scripts/deploy.sh "$(PORT)"

install:
	bash scripts/install.sh "$(PORT)" --confirm-mac "$(MAC)"

# One time, over USB: from the first (single-slot) layout to the OTA layout.
# Keeps settings, keys, bot memory and Wi-Fi; backs up nvs and store first.
relayout: firmware
	bash -c 'source scripts/esp-python.sh && "$$esp_python" scripts/relayout.py --port "$(PORT)" \
	  --images "$$images" --backup-dir .local/relayout-backups --confirm-mac "$(MAC)"'

# Every update after that, over Wi-Fi: build, sign (the household key in
# miniframework/.local/ota) and send with this build machine's deploy key.
MF := ../../microcontroller/miniframework
ifndef MASTOBOTS_OTA_VERSION
MASTOBOTS_OTA_VERSION := $(shell git rev-parse --short HEAD)$(shell git diff --quiet || echo +)-$(shell date -u +%Y%m%d%H%M%S)
endif
export MASTOBOTS_OTA_VERSION
BOTS_URL ?= https://mastomini-bots.local

ota: firmware
	cd $(MF)/crates/miniframework && cargo run -q --features ota --example ota_tool -- bundle \
	  --seed ../../.local/ota/ota.seed --app mastomini-bots --board "MASTOMINI_BOTS-BOARD:s3:mastomini-bots.local;" \
	  --chip esp32s3 --version "$(MASTOBOTS_OTA_VERSION)" \
	  C:/tmp/mmb/xtensa-esp32s3-espidf/release/mastomini-bots-esp32.bin ../../.local/ota/mastomini-bots.mfw
	bash $(MF)/tools/ota-push.sh $(BOTS_URL) $(MF)/.local/ota/mastomini-bots.mfw .env MASTOBOTS_OTA_KEY certs/household-ca.crt

boot-log:
	"$(ESP_PYTHON)" scripts/boot-log.py --port "$(PORT)"

probe-board:
	$(UV) run --project $(CLIENTTESTS) python scripts/probe-board.py --address "$(ADDRESS)"

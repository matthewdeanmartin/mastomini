SHELL := bash
.DEFAULT_GOAL := help

CARGO ?= cargo
UV ?= uv
ESP_PYTHON ?= /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe

.PHONY: help run build test lint fmt fmt-check scripts-check smoke client-test check firmware deploy install boot-log probe-board

help:
	@printf '%s\n' \
	  'make run          Start the desktop server at http://127.0.0.1:8080' \
	  'make build        Build the desktop server' \
	  'make test         Rust unit and integration tests' \
	  'make lint         Clippy with warnings as errors' \
	  'make fmt          Format Rust source' \
	  'make smoke        OAuth + posting smoke test against the real binary' \
	  'make client-test  Mastodon.py suite against the real binary (uv)' \
	  'make check        All quality gates' \
	  'make firmware                   Build the ESP32-S3 images (no board access)' \
	  'make deploy PORT=COM11          Upgrade the app on a mastomini board (DEPLOY.md)' \
	  'make install PORT=COM11 MAC=..  FIRST INSTALL ONLY: backup, erase, flash (DEPLOY.md)' \
	  'make boot-log PORT=COM11        Capture the boot log and the board address' \
	  'make probe-board ADDRESS=ip     Strict read-only check of a running board'

run:
	$(CARGO) run --locked

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

# Byte-compiles the Python tooling (deploy, probe, client tests): catches
# syntax errors in scripts that only run against real hardware.
scripts-check:
	$(UV) run --project clienttests python -m py_compile scripts/board.py scripts/boot-log.py 	  scripts/deploy.py scripts/firmware-image.py scripts/install.py scripts/probe-board.py 	  clienttests/conftest.py clienttests/harness.py clienttests/smoke.py clienttests/test_mastodon_py.py

check: fmt-check lint scripts-check test smoke client-test

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

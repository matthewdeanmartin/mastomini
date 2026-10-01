SHELL := bash
.DEFAULT_GOAL := help
CARGO ?= cargo
UV ?= uv

.PHONY: help run run-sqlite web build test check smoke firmware
help:
	@printf '%s\n' 'make run        Build Angular and start HTTP :8090 / MQTT :1883' 'make run-sqlite Optional desktop SQLite service' 'make check      Format, Clippy, Rust tests, HTTP/MQTT integration smoke' 'make firmware   Compile C6 firmware; never flashes hardware'
web:
	cd ../minicloud_ui && npm ci && npm run build
	python scripts/bundle-web.py
run: web
	$(CARGO) run --locked
run-sqlite: web
	$(CARGO) run --locked --features sqlite
build:
	$(CARGO) build --locked
test:
	$(CARGO) test --locked
smoke: build
	$(UV) run --with paho-mqtt==2.1.0 python scripts/smoke.py
check:
	$(CARGO) fmt --check
	rustfmt --edition 2021 --check src/plugins/*.rs
	$(CARGO) clippy --locked --all-targets --features sqlite -- -D warnings
	$(CARGO) test --locked --features sqlite
	$(CARGO) build --locked
	$(UV) run --with paho-mqtt==2.1.0 python scripts/smoke.py
firmware: web
	bash scripts/build-c6.sh

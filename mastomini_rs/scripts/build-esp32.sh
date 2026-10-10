#!/usr/bin/env bash
# Build the ESP32-S3 firmware image. Never opens a serial port.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -z "${MASTOMINI_WIFI_SSID:-}" || -z "${MASTOMINI_WIFI_PASSWORD:-}" ]]; then
  have_file=
  for candidate in .env ../.env; do
    if [[ -f "$candidate" ]] && grep -qE '^[[:space:]]*(export[[:space:]]+)?(MASTOMINI_)?WIFI_PASSWORD[[:space:]]*=' "$candidate"; then
      have_file=$candidate; break
    fi
  done
  if [[ -z "$have_file" ]]; then
    echo 'No built-in Wi-Fi: boards without a saved network open the mastomini-setup network.'
  else
    echo "Built-in developer Wi-Fi: $have_file (values not shown; saved networks take priority)"
  fi
fi
# MASTOMINI_BOARD=s3 (default) or p4: target, sdkconfig, build directory,
# certificate and hostname all come from that profile (scripts/board.py).
board_field() { python scripts/board.py "$1"; }
board_id=${MASTOMINI_BOARD:-s3}
target=$(board_field target)
features=$(board_field features)
# esp-idf-sys picks the chip from MCU (.cargo/config.toml defaults it to the S3).
export MCU=$(board_field chip)
export MASTOMINI_CERT_DIR="${MASTOMINI_CERT_DIR:-$(board_field cert_dir)}"
if [[ $board_id != s3 ]]; then
  # A second board must never claim the household board's name or light pin.
  export MASTOMINI_HOSTNAME="${MASTOMINI_HOSTNAME:-$(board_field hostname)}"
  export MASTOMINI_STATUS_LED_PIN="$(board_field led)"
fi
echo "Building mastomini for board $board_id ($target), certificates in $MASTOMINI_CERT_DIR"
# Git Bash support for the official Windows ESP-IDF installation (same as nanacoin).
if [[ -d /c/Espressif/frameworks/esp-idf-v5.5.3 ]]; then
  # esp-idf-sys rejects long Windows output paths; keep the target dir short.
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(board_field target_dir)}"
  export IDF_PATH="C:/Espressif/frameworks/esp-idf-v5.5.3"
  export IDF_TOOLS_PATH="C:/Espressif"
  export ESP_IDF_TOOLS_INSTALL_DIR=fromenv
  export IDF_PYTHON_ENV_PATH="C:/Espressif/python_env/idf5.5_py3.11_env"
  export ESP_ROM_ELF_DIR="C:/Espressif/tools/esp-rom-elfs/20241011"
  export PATH="/c/Espressif/frameworks/esp-idf-v5.5.3/tools:/c/Espressif/python_env/idf5.5_py3.11_env/Scripts:/c/Espressif/tools/cmake/3.30.2/bin:/c/Espressif/tools/ninja/1.12.1:/c/Espressif/tools/riscv32-esp-elf/esp-14.2.0_20251107/riscv32-esp-elf/bin:/c/Espressif/tools/xtensa-esp-elf/esp-14.2.0_20251107/xtensa-esp-elf/bin:$PATH"
  export LIBCLANG_PATH="$(cygpath -m "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin/libclang.dll"
  # ESP-IDF checks that its own GCC (esp-14.2.0_20251107) comes first on PATH;
  # rustup's bundled xtensa GCC is a fallback only, so it goes last.
  export PATH="$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin:$PATH:$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp-elf/bin"
fi
# esp-idf-sys generates its CMake project elsewhere; give it an absolute
# partition table path.
mkdir -p .embuild
# The S3 keeps its historical file name, so switching boards never forces a
# full rebuild of its ESP-IDF tree.
defaults_file=.embuild/board.defaults
[[ $board_id == s3 ]] || defaults_file=.embuild/board-$board_id.defaults
python - "$(board_field sdkconfig)" "$defaults_file" <<'PY'
import sys
from pathlib import Path
root = Path.cwd()
defaults = (root / sys.argv[1]).read_text()
defaults = defaults.replace('"partitions.csv"', '"' + (root / 'partitions.csv').as_posix() + '"')
destination = root / sys.argv[2]
if not destination.exists() or destination.read_text() != defaults:
    destination.write_text(defaults)
PY
export ESP_IDF_SDKCONFIG_DEFAULTS="$(pwd)/$defaults_file"
if command -v cygpath >/dev/null 2>&1; then
  export ESP_IDF_SDKCONFIG_DEFAULTS="$(cygpath -m "$ESP_IDF_SDKCONFIG_DEFAULTS")"
fi
# The firmware embeds the HTTPS certificate: create it if missing (never
# replaces one), and check it either way (docs/security/https.md).
bash scripts/certs.sh
# The firmware carries the household app (feature bundled-web).
bash scripts/build-web.sh
# HTTP and HTTPS are always built; HTTP2=1 adds HTTP/2 (browsers choose it
# by ALPN). Like nanacoin's.
cargo +esp build --locked --release --no-default-features --features "$features${HTTP2:+,http2}" \
  --bin mastomini-esp32 --target "$target" -Z build-std=std,panic_abort "$@"
esp_python="${MASTOMINI_ESPTOOL_PYTHON:-python}"
if [[ -z "${MASTOMINI_ESPTOOL_PYTHON:-}" && -f /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe ]]; then
  esp_python=/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe
fi
"$esp_python" scripts/firmware-image.py "${CARGO_TARGET_DIR:-target}/$target/release/mastomini-esp32"

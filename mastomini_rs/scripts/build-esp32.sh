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
# Git Bash support for the official Windows ESP-IDF installation (same as nanacoin).
if [[ -d /c/Espressif/frameworks/esp-idf-v5.5.3 ]]; then
  # esp-idf-sys rejects long Windows output paths; keep the target dir short.
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-C:/mmr}"
  export IDF_PATH="C:/Espressif/frameworks/esp-idf-v5.5.3"
  export IDF_TOOLS_PATH="C:/Espressif"
  export ESP_IDF_TOOLS_INSTALL_DIR=fromenv
  export IDF_PYTHON_ENV_PATH="C:/Espressif/python_env/idf5.5_py3.11_env"
  export ESP_ROM_ELF_DIR="C:/Espressif/tools/esp-rom-elfs/20241011"
  export PATH="/c/Espressif/frameworks/esp-idf-v5.5.3/tools:/c/Espressif/python_env/idf5.5_py3.11_env/Scripts:/c/Espressif/tools/cmake/3.30.2/bin:/c/Espressif/tools/ninja/1.12.1:/c/Espressif/tools/xtensa-esp-elf/esp-14.2.0_20251107/xtensa-esp-elf/bin:$PATH"
  export LIBCLANG_PATH="$(cygpath -m "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin/libclang.dll"
  # ESP-IDF checks that its own GCC (esp-14.2.0_20251107) comes first on PATH;
  # rustup's bundled xtensa GCC is a fallback only, so it goes last.
  export PATH="$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin:$PATH:$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp-elf/bin"
fi
# esp-idf-sys generates its CMake project elsewhere; give it an absolute
# partition table path.
mkdir -p .embuild
python - <<'PY'
from pathlib import Path
root = Path.cwd()
defaults = (root / 'sdkconfig.defaults').read_text()
defaults = defaults.replace('"partitions.csv"', '"' + (root / 'partitions.csv').as_posix() + '"')
destination = root / '.embuild/board.defaults'
if not destination.exists() or destination.read_text() != defaults:
    destination.write_text(defaults)
PY
export ESP_IDF_SDKCONFIG_DEFAULTS="$(pwd)/.embuild/board.defaults"
if command -v cygpath >/dev/null 2>&1; then
  export ESP_IDF_SDKCONFIG_DEFAULTS="$(cygpath -m "$ESP_IDF_SDKCONFIG_DEFAULTS")"
fi
# The firmware carries the household app (feature bundled-web).
bash scripts/build-web.sh
cargo +esp build --locked --release --no-default-features --features esp32 \
  --bin mastomini-esp32 --target xtensa-esp32s3-espidf -Z build-std=std,panic_abort "$@"
esp_python="${MASTOMINI_ESPTOOL_PYTHON:-python}"
if [[ -z "${MASTOMINI_ESPTOOL_PYTHON:-}" && -f /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe ]]; then
  esp_python=/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe
fi
"$esp_python" scripts/firmware-image.py "${CARGO_TARGET_DIR:-target}/xtensa-esp32s3-espidf/release/mastomini-esp32"

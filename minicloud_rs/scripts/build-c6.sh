#!/usr/bin/env bash
# Compile only. No serial port, erase, format or flash commands.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ${MINICLOUD_CONFIG_LOADED:-0} != 1 && -f .env ]]; then
  task_bash="$BASH"
  if command -v cygpath >/dev/null 2>&1; then task_bash="$(cygpath -m "$task_bash")"; fi
  exec python scripts/with-config.py "$task_bash" scripts/build-c6.sh
fi
: "${MINICLOUD_WIFI_SSID:?Set MINICLOUD_WIFI_SSID}"
: "${MINICLOUD_WIFI_PASSWORD:?Set MINICLOUD_WIFI_PASSWORD}"
: "${MINICLOUD_ADMIN_TOKEN:?Set MINICLOUD_ADMIN_TOKEN (24+ characters)}"
if [[ ${#MINICLOUD_ADMIN_TOKEN} -lt 24 ]]; then echo 'Admin token must be 24+ characters' >&2; exit 1; fi
if [[ -d /c/Espressif/frameworks/esp-idf-v5.5.3 ]]; then
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-C:/mcr}"
  export IDF_PATH="C:/Espressif/frameworks/esp-idf-v5.5.3"
  export IDF_TOOLS_PATH="C:/Espressif"
  export ESP_IDF_TOOLS_INSTALL_DIR=fromenv
  export IDF_PYTHON_ENV_PATH="C:/Espressif/python_env/idf5.5_py3.11_env"
  export ESP_ROM_ELF_DIR="C:/Espressif/tools/esp-rom-elfs/20241011"
  export LIBCLANG_PATH="$(cygpath -m "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin/libclang.dll"
  export PATH="/c/Espressif/frameworks/esp-idf-v5.5.3/tools:/c/Espressif/tools/riscv32-esp-elf/esp-14.2.0_20251107/riscv32-esp-elf/bin:/c/Espressif/tools/cmake/3.30.2/bin:/c/Espressif/tools/ninja/1.12.1:/c/Espressif/python_env/idf5.5_py3.11_env/Scripts:$(cygpath -u "$USERPROFILE")/.rustup/toolchains/esp/xtensa-esp32-elf-clang/esp-clang/bin:$PATH"
fi
mkdir -p .embuild
python - <<'PY'
from pathlib import Path
import hashlib
root=Path.cwd()
text=(root/'sdkconfig.defaults').read_text().replace('"partitions.csv"','"'+(root/'partitions.csv').as_posix()+'"')
# esp-idf-sys watches sdkconfig defaults and binding headers, but does not
# watch extra-component C sources. Include their content digest in this
# watched input so Cargo reruns IDF/Ninja whenever the LCD driver changes.
digest=hashlib.sha256()
for path in sorted((root/'components/display').rglob('*')):
    if path.is_file():
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b'\0')
        digest.update(path.read_bytes())
        digest.update(b'\0')
text+='\n# Minicloud display component SHA-256: '+digest.hexdigest()+'\n'
defaults=root/'.embuild/c6.defaults'
if not defaults.exists() or defaults.read_text() != text:
    defaults.write_text(text)
PY
export ESP_IDF_SDKCONFIG_DEFAULTS="$(pwd)/.embuild/c6.defaults"
if command -v cygpath >/dev/null 2>&1; then export ESP_IDF_SDKCONFIG_DEFAULTS="$(cygpath -m "$ESP_IDF_SDKCONFIG_DEFAULTS")"; fi
cargo +esp build --locked --release --no-default-features --features esp32 --bin minicloud-c6 --target riscv32imac-esp-espidf -Z build-std=std,panic_abort
if [[ -f /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe ]]; then
  export MINICLOUD_ESP_PYTHON='C:/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe'
fi
python scripts/firmware-report.py

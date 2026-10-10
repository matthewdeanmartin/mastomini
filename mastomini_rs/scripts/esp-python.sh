# Sourced by the deploy scripts: pick the ESP-IDF Python that has esptool 4.x.
# Unbuffered, so script messages interleave correctly with esptool output.
export PYTHONUNBUFFERED=1
esp_python="${MASTOMINI_ESPTOOL_PYTHON:-python}"
if [[ -z "${MASTOMINI_ESPTOOL_PYTHON:-}" && -f /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe ]]; then
  esp_python=/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe
fi
# MASTOMINI_BOARD (s3 default, p4) picks the build directory and target.
if [[ -d /c/Espressif/frameworks/esp-idf-v5.5.3 ]]; then
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(python scripts/board.py target_dir)}"
fi
images="${CARGO_TARGET_DIR:-target}/$(python scripts/board.py target)/release"

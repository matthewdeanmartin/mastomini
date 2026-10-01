"""Guarded USB deployment of the identified non-Touch C6. Run from Git Bash.

--replace erases the entire board and installs an empty SPIFFS image.
--update preserves NVS and SPIFFS; use only on a working Minicloud installation.
Neither mode builds or flashes another MCU type.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
IDF = Path(os.environ.get('IDF_PATH', 'C:/Espressif/frameworks/esp-idf-v5.5.3'))
ESP_PYTHON = os.environ.get('MINICLOUD_ESP_PYTHON', 'C:/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe' if os.name == 'nt' else sys.executable)


def run(args):
    subprocess.run(args, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', required=True)
    parser.add_argument('--expected-mac', default='ac:eb:e6:1e:13:40')
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--replace', action='store_true')
    mode.add_argument('--update', action='store_true')
    args = parser.parse_args()
    manifest = json.loads((ROOT/'.embuild/deployment-images.json').read_text())
    if manifest['chip'] != 'esp32c6' or manifest['flash_bytes'] != 0x800000:
        raise SystemExit('Refusing unexpected firmware target/flash size')
    images = manifest['images']
    if [i['address'] for i in images] != [0, 0x8000, 0x10000]:
        raise SystemExit('Refusing unexpected image offsets')
    for i, limit in zip(images, [0x8000, 0x1000, 0x200000]):
        data = Path(i['path']).read_bytes()
        if len(data) != i['size'] or len(data) > limit or hashlib.sha256(data).hexdigest() != i['sha256']:
            raise SystemExit('Image changed or exceeds its partition; rebuild before flashing')
    table = Path(images[1]['path']).read_bytes()
    partitions = {}
    for offset in range(0, len(table), 32):
        row = table[offset:offset+32]
        if len(row) < 32 or row[:2] != b'\xaaP':
            break
        _, kind, subtype, address, size, label, flags = struct.unpack('<HBBII16sI', row)
        partitions[label.split(b'\0')[0].decode()] = (kind, subtype, address, size, flags)
    expected = {'nvs': (1,2,0x9000,0x6000,0), 'phy_init': (1,1,0xf000,0x1000,0),
                'factory': (0,0,0x10000,0x200000,0), 'storage': (1,0x82,0x210000,0x5f0000,0)}
    if partitions != expected:
        raise SystemExit('Refusing unexpected partition table')
    # Verify identity before any erase or write. Supplying --chip also refuses
    # an S2/S3 even if it happens to appear on the requested COM port.
    base = [ESP_PYTHON, '-m', 'esptool', '--chip', 'esp32c6', '--port', args.port]
    result = subprocess.run(base+['flash_id'], check=True, capture_output=True, text=True)
    print(result.stdout, end='')
    found = re.search(r'^BASE MAC:\s*([0-9a-f:]+)', result.stdout, re.M | re.I)
    if not found or found.group(1).lower() != args.expected_mac.lower() or 'Detected flash size: 8MB' not in result.stdout:
        raise SystemExit('Board MAC/8MB flash does not match the selected unit; nothing written')
    flash = []
    for image in images:
        flash += [hex(image['address']), image['path']]
    if args.replace:
        empty = ROOT/'.embuild/empty-spiffs'
        empty.mkdir(exist_ok=True)
        if any(empty.iterdir()):
            raise SystemExit('Empty SPIFFS source directory contains files; nothing written')
        storage = ROOT/'.embuild/empty-storage.bin'
        run([ESP_PYTHON, str(IDF/'components/spiffs/spiffsgen.py'), '0x5f0000', str(empty), str(storage),
             '--page-size','256','--block-size','4096','--obj-name-len','32','--meta-len','4','--use-magic','--use-magic-len'])
        if storage.stat().st_size != 0x5f0000:
            raise SystemExit('SPIFFS image has unexpected size; nothing written')
        print('Replacing this board: erase all flash, install Minicloud and empty storage.', flush=True)
        run(base+['erase_flash'])
        flash += ['0x210000', str(storage)]
    else:
        print('Updating Minicloud firmware; retaining NVS and SPIFFS.', flush=True)
    run(base+['--baud','460800','write_flash','--flash_size','8MB','--flash_mode','dio','--flash_freq','80m']+flash)
    # write_flash verifies every written region and hard-resets the board.
    print('Flash verified and board reset. Check serial boot, HTTP, MQTT and LCD before declaring deployment complete.')


if __name__ == '__main__':
    main()

"""Shared board helpers for deploy.py and install.py (esptool 4.x)."""
from __future__ import annotations

import pathlib
import re
import struct
import subprocess
import sys
import tempfile

# (type, subtype, offset, size): must match partitions.csv exactly.
MASTOMINI_LAYOUT = {
    'nvs': (1, 0x02, 0x9000, 0x6000),
    'phy_init': (1, 0x01, 0xF000, 0x1000),
    'factory': (0, 0x00, 0x10000, 0x400000),
    'store': (1, 0x02, 0x410000, 0x800000),
    'media': (1, 0x82, 0xC10000, 0x300000),
    'coredump': (1, 0x03, 0xF10000, 0x10000),
}
FLASH_SIZE = 0x1000000
APP_LIMIT = 0x400000


def esptool(port: str) -> list[str]:
    # `--after watchdog_reset`: on this board esptool's default USB-Serial/JTAG
    # hard reset leaves the chip latched in download mode, so the firmware
    # never starts. An RTC watchdog reset clears the latch and boots the app.
    return [sys.executable, '-m', 'esptool', '--chip', 'esp32s3', '--port', port,
            '--after', 'watchdog_reset']


def parse_table(data: bytes) -> dict[str, tuple[int, int, int, int]]:
    found = {}
    for at in range(0, min(len(data), 0xC00), 32):
        row = data[at:at + 32]
        magic, = struct.unpack_from('<H', row)
        if magic in (0xFFFF, 0xEBEB):  # end of table / MD5 trailer
            break
        if magic != 0x50AA:
            raise ValueError('No valid partition table at 0x8000')
        _, kind, subtype, offset, size, label, flags = struct.unpack('<HBBII16sI', row)
        name = label.rstrip(b'\0').decode('ascii', 'replace')
        if flags or name in found:
            raise ValueError('Encrypted or duplicate partitions: needs a separate review')
        found[name] = (kind, subtype, offset, size)
    return found


def read_table(port: str) -> dict[str, tuple[int, int, int, int]]:
    with tempfile.TemporaryDirectory(prefix='mastomini-pt-') as temp:
        path = pathlib.Path(temp) / 'pt.bin'
        subprocess.run([*esptool(port), 'read_flash', '0x8000', '0x1000', str(path)], check=True)
        return parse_table(path.read_bytes())


def describe(table: dict[str, tuple[int, int, int, int]]) -> str:
    return '\n'.join(f'  {n:<10} type={k} subtype=0x{s:02x} offset=0x{o:06x} size=0x{z:06x}'
                     for n, (k, s, o, z) in table.items()) or '  (empty)'


def read_mac(port: str) -> str:
    out = subprocess.run([*esptool(port), 'read_mac'], check=True, capture_output=True, text=True).stdout
    match = re.search(r'MAC:\s*([0-9a-f:]{17})', out, re.I)
    if not match:
        raise SystemExit('Could not read the board MAC')
    return match.group(1).lower()


def check_image(path: pathlib.Path, limit: int) -> None:
    if not path.is_file() or not 0 < path.stat().st_size <= limit:
        raise SystemExit(f'Missing or oversized image: {path}. Build firmware first.')

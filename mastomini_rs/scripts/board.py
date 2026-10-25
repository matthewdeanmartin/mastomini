"""Shared board helpers for deploy.py and install.py (esptool 4.x).

MASTOMINI_BOARD picks the board profile: s3 (default, the household's
ESP32-S3) or p4 (an ESP32-P4 with an ESP32-C6 Wi-Fi companion). The profile
decides the chip, Rust target, flash size, bootloader offset, build and
certificate directories and hostname, so one board's image or certificate
is never used for the other. Shell scripts read fields with
`python scripts/board.py <field>`.
"""
from __future__ import annotations

import os
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
PROFILES = {
    # `after`: on the S3's native USB-Serial/JTAG, esptool's default hard reset
    # leaves the chip latched in download mode; an RTC watchdog reset boots it.
    # The P4-WIFI6 is flashed through a CH343 bridge, whose DTR/RTS reset works.
    's3': dict(chip='esp32s3', target='xtensa-esp32s3-espidf', flash_size='16MB',
               flash_bytes=0x1000000, bootloader=0x0, after='watchdog_reset',
               target_dir='C:/tmp/mmr', cert_dir='certs', hostname='mastomini',
               sdkconfig='sdkconfig.defaults', features='esp32', led='', toolchain='xtensa'),
    'p4': dict(chip='esp32p4', target='riscv32imafc-esp-espidf', flash_size='32MB',
               flash_bytes=0x2000000, bootloader=0x2000, after='hard_reset',
               target_dir='C:/tmp/mmr-p4', cert_dir='certs-p4', hostname='mastomini-p4',
               sdkconfig='boards/p4/sdkconfig.defaults', features='esp32,board-p4', led='off',
               toolchain='riscv'),
}
BOARD_ID = os.environ.get('MASTOMINI_BOARD', 's3')
if BOARD_ID not in PROFILES:
    raise SystemExit(f'MASTOMINI_BOARD must be one of {", ".join(PROFILES)}, not {BOARD_ID!r}')
PROFILE = PROFILES[BOARD_ID]
FLASH_SIZE = PROFILE['flash_bytes']
APP_LIMIT = 0x400000


def esptool(port: str) -> list[str]:
    return [sys.executable, '-m', 'esptool', '--chip', PROFILE['chip'], '--port', port,
            '--after', PROFILE['after']]


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


if __name__ == '__main__':
    # Shell helper: python scripts/board.py <field>, for the MASTOMINI_BOARD profile.
    if len(sys.argv) != 2 or sys.argv[1] not in PROFILE:
        raise SystemExit(f'Usage: python scripts/board.py <{"|".join(PROFILE)}>')
    print(PROFILE[sys.argv[1]])

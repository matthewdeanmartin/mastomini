"""FIRST INSTALL of mastomini on a board that has never run it.

Destroys whatever the board holds. Safety rails:
  * refuses a board that already has the mastomini layout (that would wipe a
    household; use deploy.py to upgrade instead);
  * requires --confirm-mac with the MAC of the attached board, so the operator
    has identified the physical board;
  * always reads a full 16 MiB backup first and verifies its size before any
    write.

Then it erases nvs, store, media and coredump and writes the bootloader
(0x0), partition table (0x8000) and application (0x10000).
"""
import argparse
import datetime
import hashlib
import pathlib
import subprocess

from board import (APP_LIMIT, FLASH_SIZE, MASTOMINI_LAYOUT, check_image, describe, esptool,
                   read_mac, read_table)

ERASE = ['nvs', 'store', 'media', 'coredump']


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--port', required=True)
    parser.add_argument('--images', type=pathlib.Path, required=True,
                        help='directory with mastomini-esp32.bin, mastomini-partitions.bin, mastomini-bootloader.bin')
    parser.add_argument('--backup-dir', type=pathlib.Path, required=True)
    parser.add_argument('--confirm-mac', help='MAC of the attached board, e.g. ac:a7:04:2c:29:9c')
    parser.add_argument('--dry-run', action='store_true', help='Identify the board and print the plan; no writes')
    args = parser.parse_args()

    app = args.images / 'mastomini-esp32.bin'
    table_bin = args.images / 'mastomini-partitions.bin'
    boot = args.images / 'mastomini-bootloader.bin'
    check_image(app, APP_LIMIT)
    check_image(table_bin, 0x1000)
    check_image(boot, 0x8000 - 0x0)

    mac = read_mac(args.port)
    table = read_table(args.port)
    print(f'Board on {args.port}: MAC {mac}')
    print(f'Current partition table:\n{describe(table)}')
    if table == MASTOMINI_LAYOUT:
        raise SystemExit('REFUSED: this board already runs mastomini. Installing would erase the household.\n'
                         'Use scripts/deploy.sh to upgrade it.')
    print('Plan: full backup, erase ' + ', '.join(ERASE) + ', write bootloader/partition table/application.')
    if args.dry_run:
        print(f'Dry run. To install, rerun with --confirm-mac {mac}')
        return
    if (args.confirm_mac or '').lower() != mac:
        raise SystemExit(f'REFUSED: --confirm-mac must be {mac} (the attached board).')

    args.backup_dir.mkdir(parents=True, exist_ok=True)
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S')
    backup = args.backup_dir / f'{mac.replace(":", "")}-{stamp}-full16MB.bin'
    print(f'Backing up the whole flash to {backup} (about 4 minutes)...')
    subprocess.run([*esptool(args.port), '-b', '921600', 'read_flash', '0', hex(FLASH_SIZE), str(backup)],
                   check=True)
    data = backup.read_bytes()
    if len(data) != FLASH_SIZE:
        raise SystemExit('Backup is incomplete; nothing was written.')
    print(f'Backup sha256 {hashlib.sha256(data).hexdigest()}')

    tool = esptool(args.port)
    for name in ERASE:
        _, _, offset, size = MASTOMINI_LAYOUT[name]
        subprocess.run([*tool, 'erase_region', hex(offset), hex(size)], check=True)
    subprocess.run([*tool, 'write_flash', '--flash_mode', 'dio', '--flash_size', '16MB',
                    '0x0', str(boot), '0x8000', str(table_bin), '0x10000', str(app)], check=True)
    if read_table(args.port) != MASTOMINI_LAYOUT:
        raise SystemExit('Partition table did not verify after writing. Restore from the backup.')
    print('Installed. The board restarts unprovisioned.')


if __name__ == '__main__':
    main()

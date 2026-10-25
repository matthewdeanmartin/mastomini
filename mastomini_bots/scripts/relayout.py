"""ONE-TIME RE-LAYOUT of a mastomini-bots board to the OTA layout.

Only from exactly the first layout (BOTS_LAYOUT_V1, partitions-v1.csv). Every
partition of it keeps its offset and size, so the bots' settings, API keys and
memory (`store`), Wi-Fi (`nvs`) and the crash record stay where they are; the
new layout adds `otadata` and `ota_1` in flash the first layout left empty.

Safety rails:
  * refuses a mastomini board (the household server), by layout and by MAC;
  * requires --confirm-mac with the MAC of the attached board;
  * requires exactly the first layout (anything else: refused, nothing written);
  * backs up `nvs` and `store` first and checks the backup's size;
  * erases only `otadata` (new, so it starts blank: boot the first slot);
  * writes the bootloader (rollback-capable), the new table and the app, then
    reads the table back.
"""
import argparse
import datetime
import hashlib
import pathlib
import subprocess

from board import (APP_LIMIT, BOOTLOADER, BOTS_LAYOUT, BOTS_LAYOUT_V1, IMAGE, TABLE, check_image, describe,
                   esptool, read_mac, read_table, refuse_mastomini)

BACKUP = ['nvs', 'store']


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--port', required=True)
    parser.add_argument('--images', type=pathlib.Path, required=True,
                        help=f'directory with {IMAGE}, {TABLE}, {BOOTLOADER}')
    parser.add_argument('--backup-dir', type=pathlib.Path, required=True)
    parser.add_argument('--confirm-mac', help='MAC of the attached board')
    parser.add_argument('--dry-run', action='store_true', help='Identify the board and print the plan; no writes')
    args = parser.parse_args()

    app, table_bin, boot = args.images / IMAGE, args.images / TABLE, args.images / BOOTLOADER
    check_image(app, APP_LIMIT)
    check_image(table_bin, 0x1000)
    check_image(boot, 0x8000)

    mac = read_mac(args.port, after='no_reset')
    table = read_table(args.port, after='no_reset')
    print(f'Board on {args.port}: MAC {mac}')
    print(f'Current partition table:\n{describe(table)}')
    refuse_mastomini(mac, table)
    if table == BOTS_LAYOUT:
        raise SystemExit('Nothing to do: this board already has the OTA layout. Use make ota (or make deploy).')
    if table != BOTS_LAYOUT_V1:
        raise SystemExit('REFUSED: this is not the first mastomini-bots layout; a re-layout from it needs its own review.')
    print(f'Plan: back up {", ".join(BACKUP)}; erase otadata only; write bootloader, OTA table and app.')
    print('Settings, API keys, bot memory (store), Wi-Fi (nvs) and coredump are not erased or moved.')
    if args.dry_run:
        print(f'Dry run. To re-layout, rerun with --confirm-mac {mac}')
        return
    if (args.confirm_mac or '').lower() != mac:
        raise SystemExit(f'REFUSED: --confirm-mac must be {mac} (the attached board).')

    tool = esptool(args.port, after='no_reset')
    args.backup_dir.mkdir(parents=True, exist_ok=True)
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S')
    for name in BACKUP:
        _, _, offset, size = BOTS_LAYOUT_V1[name]
        path = args.backup_dir / f'{mac.replace(":", "")}-{stamp}-{name}-{offset:#x}.bin'
        subprocess.run([*tool, '-b', '921600', 'read_flash', hex(offset), hex(size), str(path)], check=True)
        data = path.read_bytes()
        if len(data) != size:
            raise SystemExit(f'Backup of {name} is incomplete; nothing was written.')
        print(f'Backed up {name}: {path} sha256 {hashlib.sha256(data).hexdigest()}')

    _, _, offset, size = BOTS_LAYOUT['otadata']
    subprocess.run([*tool, 'erase_region', hex(offset), hex(size)], check=True)
    subprocess.run([*tool, 'write_flash', '--flash_mode', 'dio', '--flash_size', '16MB',
                    '0x0', str(boot), '0x8000', str(table_bin), '0x10000', str(app)], check=True)
    if read_table(args.port) != BOTS_LAYOUT:
        raise SystemExit('The new partition table did not read back. The data partitions were not touched; '
                         'restore the old table with the v1 build (scripts/install.py is NOT the way: it erases).')
    print('Re-laid out. The board restarts on ota_0; later updates: make ota.')


if __name__ == '__main__':
    main()

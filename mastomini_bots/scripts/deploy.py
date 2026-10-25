"""Upgrade the application on a board that already runs mastomini-bots.

Writes ONLY the application at 0x10000 (and blanks otadata, so it boots), after checking that the board has
exactly the mastomini-bots partition layout and is not a mastomini board.
The bots' settings, API keys and memory (the `store` NVS partition), Wi-Fi
config, bootloader and partition table are never written. No erase, no
recovery, no serial monitor.
"""
import argparse
import pathlib
import subprocess

from board import (APP_LIMIT, BOTS_LAYOUT, BOTS_LAYOUT_V1, check_image, describe, esptool, read_mac, read_table,
                   refuse_mastomini)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', required=True)
    parser.add_argument('--image', type=pathlib.Path, required=True)
    parser.add_argument('--dry-run', action='store_true', help='Print the plan without opening the port')
    args = parser.parse_args()
    image = args.image.resolve()
    check_image(image, APP_LIMIT)
    if image.read_bytes()[:1] != b'\xe9':
        parser.error('Not an Espressif application image')
    print(f'Plan: check that the board on {args.port} is a mastomini-bots board (not mastomini), '
          f'then write {image} at 0x10000 only.')
    print('Bot settings, API keys, Wi-Fi config, bootloader and partition table will not be written.')
    if args.dry_run:
        return
    mac = read_mac(args.port)
    table = read_table(args.port)
    refuse_mastomini(mac, table)
    if table == BOTS_LAYOUT_V1:
        raise SystemExit('REFUSED: this board has the first (single-slot) layout and this build is for the OTA layout.\n'
                         'Move it once with make relayout PORT=... (keeps settings and keys).')
    if table != BOTS_LAYOUT:
        raise SystemExit('REFUSED: the board does not have the mastomini-bots partition layout.\n'
                         f'Found:\n{describe(table)}\n'
                         'Use the first-install procedure (DEPLOY.md) only for a board that has never run mastomini-bots.')
    print(f'Board {mac} has the mastomini-bots layout.')
    _, _, ota_offset, ota_size = BOTS_LAYOUT['otadata']
    # Boot the slot written here (ota_0), even if a network update had moved
    # the board to ota_1: a blank otadata means "the first slot".
    subprocess.run([*esptool(args.port, after='no_reset'), 'erase_region', hex(ota_offset), hex(ota_size)], check=True)
    subprocess.run([*esptool(args.port), 'write_flash', '--flash_mode', 'dio', '--flash_size', '16MB',
                    '0x10000', str(image)], check=True)
    print('Application updated. The board restarts; bot settings and keys were not touched.')


if __name__ == '__main__':
    main()

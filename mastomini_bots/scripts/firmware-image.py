"""Offline conversion and size gate. Run with the ESP-IDF Python (esptool 4.x).

Produces, next to the ELF:
  mastomini-bots-esp32.bin        application image (0x10000)
  mastomini-bots-partitions.bin   partition table from partitions.csv (0x8000)
  mastomini-bots-bootloader.bin   second-stage bootloader (0x0), first install only
No board is accessed.
"""
import os
import pathlib
import shutil
import subprocess
import sys

APP_LIMIT = 0x400000
CRATE = pathlib.Path(__file__).resolve().parent.parent


def main():
    elf = pathlib.Path(sys.argv[1]).resolve()
    out = elf.parent
    image = elf.with_suffix('.bin')
    subprocess.run([sys.executable, '-m', 'esptool', '--chip', 'esp32s3',
                    'elf2image', '--flash_mode', 'dio', '--flash_size', '16MB',
                    '--output', str(image), str(elf)], check=True)
    size = image.stat().st_size
    if not 0 < size <= APP_LIMIT:
        raise SystemExit(f'Firmware is {size} bytes; factory partition is {APP_LIMIT}. Refusing.')

    idf = pathlib.Path(os.environ.get('IDF_PATH', 'C:/Espressif/frameworks/esp-idf-v5.5.3'))
    table = out / 'mastomini-bots-partitions.bin'
    subprocess.run([sys.executable, str(idf / 'components/partition_table/gen_esp32part.py'),
                    '--flash-size', '16MB', str(CRATE / 'partitions.csv'), str(table)], check=True)

    # esp-idf-sys builds the bootloader inside its CMake output directory.
    candidates = sorted(out.glob('build/esp-idf-sys-*/out/build/bootloader/bootloader.bin'),
                        key=lambda p: p.stat().st_mtime)
    if not candidates:
        raise SystemExit('bootloader.bin not found in the esp-idf-sys build output')
    shutil.copyfile(candidates[-1], out / 'mastomini-bots-bootloader.bin')
    print(f'Application: {image} ({size} / {APP_LIMIT} bytes)')
    print(f'Partition table: {table}')
    print(f'Bootloader: {out / "mastomini-bots-bootloader.bin"}')
    print('No board accessed.')


if __name__ == '__main__':
    main()

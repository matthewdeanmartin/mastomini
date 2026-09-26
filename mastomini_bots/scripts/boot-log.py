"""Capture the bots board's boot log from the USB serial port.

Adapted from ../mastomini_rs/scripts/boot-log.py. By default the board is
restarted first with esptool's RTC watchdog reset (esptool's plain USB reset
leaves these boards stuck in download mode). The restart does not write
flash. Pass --no-reset to only listen.

Prints the log, then a SUMMARY line. Exit codes: 0 the firmware reached
"Ready at"; 2 the build has no Wi-Fi credentials; 3 it could not join Wi-Fi
(it keeps retrying); 1 anything else. Run with the ESP-IDF Python:
    /c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe scripts/boot-log.py --port COM12
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
import time

import serial


def restart(port: str) -> None:
    subprocess.run([sys.executable, '-m', 'esptool', '--chip', 'esp32s3', '--port', port,
                    '--after', 'watchdog_reset', 'read_mac'],
                   check=True, stdout=subprocess.DEVNULL)


def open_port(name: str) -> serial.Serial:
    port = serial.Serial()
    port.port = name
    port.baudrate = 115200
    port.timeout = 0.2
    # Both lines low: opening must not reset the chip or select download mode.
    port.dtr = False
    port.rts = False
    for _ in range(50):  # the port reappears a moment after the reset
        try:
            port.open()
            return port
        except serial.SerialException:
            time.sleep(0.1)
    raise SystemExit(f'Could not open {name}; close any serial monitor using it')


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--port', required=True)
    parser.add_argument('--seconds', type=float, default=90)
    parser.add_argument('--no-reset', action='store_true')
    args = parser.parse_args()

    if not args.no_reset:
        restart(args.port)
    port = open_port(args.port)
    deadline = time.monotonic() + args.seconds
    text = ''
    while time.monotonic() < deadline:
        chunk = port.read(4096).decode('utf-8', 'replace')
        if chunk:
            sys.stdout.write(chunk)
            sys.stdout.flush()
            text += chunk
            if 'Ready at' in text or 'No Wi-Fi in this build' in text:
                deadline = min(deadline, time.monotonic() + 2)
    port.close()

    # "Ready at https://mastomini-bots.local/app/ (https://192.168.1.170/app/)"
    ready = re.search(r'Ready at \S+ \(https?://([\d.]+)/', text)
    if ready:
        print(f'\nSUMMARY: ready, address {ready.group(1)}')
        return 0
    if 'No Wi-Fi in this build' in text:
        print('\nSUMMARY: no Wi-Fi credentials in this build (MASTOMINI_WIFI_SSID); see DEPLOY.md step 1')
        return 2
    joins = re.findall(r'Could not join (\S+) \(([^)]*)\)', text)
    if joins:
        ssid, why = joins[-1]
        print(f'\nSUMMARY: Wi-Fi: could not join {ssid} ({why}) in {len(joins)} attempts; '
              'the board keeps retrying (see DEPLOY.md step 5)')
        return 3
    if 'waiting for download' in text:
        print('\nSUMMARY: the chip is in download mode (firmware not running); see DEPLOY.md step 5')
        return 1
    crashed = re.search(r'panic|abort\(\) was called|Guru Meditation|Error: ', text)
    reason = 'the firmware reported an error' if crashed else 'nothing more within the time limit'
    print(f'\nSUMMARY: boot did not reach "Ready at" ({reason}); see the log above')
    return 1


if __name__ == '__main__':
    sys.exit(main())

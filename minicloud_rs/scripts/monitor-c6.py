"""Bounded serial boot capture; optional application reset, never flashes."""
import argparse
from pathlib import Path
import time
import serial
from esptool.reset import HardReset

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--port',required=True)
parser.add_argument('--seconds',type=int,default=60)
parser.add_argument('--reset',action='store_true')
args = parser.parse_args()
if not 1 <= args.seconds <= 600:
    raise SystemExit('Capture duration must be 1..600 seconds')
root = Path(__file__).resolve().parents[1]
log = root/'.embuild/board-serial.log'
device = serial.Serial(port=None,baudrate=115200,timeout=.25)
device.dtr = False; device.rts = False; device.port = args.port
with device, log.open('wb') as output:
    if args.reset:
        HardReset(device,uses_usb=True)()
    end = time.monotonic()+args.seconds
    while time.monotonic() < end:
        data = device.read(4096)
        if data:
            output.write(data); output.flush()
            print(data.decode('utf-8',errors='replace'),end='',flush=True)
print('\nSerial capture saved to',log)

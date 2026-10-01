"""Make an app image and report flash/static RAM fit. Never opens a port."""
import os
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
import subprocess
import sys

root=Path(__file__).resolve().parents[1]
target=Path(os.environ.get("CARGO_TARGET_DIR","target")) / "riscv32imac-esp-espidf/release"
elf=target / "minicloud-c6"
output=root / ".embuild/minicloud-c6.bin"
python=os.environ.get("MINICLOUD_ESP_PYTHON",sys.executable)
subprocess.run([python,"-m","esptool","--chip","esp32c6","elf2image","--flash_size","8MB","--flash_mode","dio","--flash_freq","80m","--output",str(output),str(elf)],check=True)
size=output.stat().st_size
print(f"C6 app image: {size:,} / {0x200000:,} bytes ({size/0x200000:.1%} of factory partition)")
if size > 0x200000:
    raise SystemExit("Image exceeds the existing 2 MiB application partition")
print(f"Bootloader: {target/'bootloader.bin'}")
print(f"Partition table: {target/'partition-table.bin'}")
images = []
for address, path in [(0, target/'bootloader.bin'), (0x8000, target/'partition-table.bin'), (0x10000, output)]:
    images.append(dict(address=address, path=str(path.resolve()), size=path.stat().st_size,
                       sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
(root/'.embuild/deployment-images.json').write_text(json.dumps(dict(
    chip='esp32c6', flash_bytes=8*1024*1024,
    created_at=datetime.now(timezone.utc).isoformat(), images=images), indent=2)+'\n')
print(f"Deployment image manifest: {root/'.embuild/deployment-images.json'}")
print("Runtime heap fit is unverified until hardware testing. No hardware was changed.")

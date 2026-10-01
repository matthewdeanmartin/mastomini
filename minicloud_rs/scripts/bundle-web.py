"""Embed only gzip assets; no compressor or full UI buffer on the board."""
import gzip
from pathlib import Path

root = Path(__file__).resolve().parents[1]
source = root.parent / "minicloud_ui/dist/minicloud-ui/browser"
destination = root / "web"
destination.mkdir(exist_ok=True)
for old in destination.glob("*.gz"):
    old.unlink()
total = 0
for file in source.iterdir():
    if file.is_file() and file.suffix in {".html", ".js", ".css"}:
        data = gzip.compress(file.read_bytes(), compresslevel=9, mtime=0)
        (destination / (file.name + ".gz")).write_bytes(data)
        total += len(data)
print(f"Bundled Angular: {total:,} gzip bytes in read-only app flash")
if total > 180_000:
    raise SystemExit("Angular exceeds the 180 KB compressed UI budget")

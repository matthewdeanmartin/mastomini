"""Repeatable authenticated read bursts against a disposable desktop store.

Run after cargo build --release --locked. Seeding/authentication are excluded
from timings. Never uses or mutates an existing household or board.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import importlib.util
import hashlib
import json
from pathlib import Path
import platform
import sys

CRATE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CRATE / "clienttests"))
from harness import ServerProcess  # noqa: E402
from conftest import sign_in  # noqa: E402

spec = importlib.util.spec_from_file_location("bench_api", CRATE / "scripts/bench-api.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=CRATE / "target/release" / ("mastomini.exe" if sys.platform == "win32" else "mastomini"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=10)
    args = parser.parse_args()
    if not 2 <= args.rounds <= 1000:
        parser.error("rounds must be 2..1000")
    server = ServerProcess(executable=args.binary.resolve())
    output = {"created_utc": datetime.now(timezone.utc).isoformat(),
              "workload": "40 public posts, one account, real OAuth, FileStore, HTTP loopback",
              "binary": args.binary.name, "password_rounds": 1000,
              "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
              "platform": platform.platform(), "python": platform.python_version(),
              "note": "Desktop HTTP only; not ESP32/TLS timing. Authentication and seeding excluded.",
              "results": []}
    failed = False
    try:
        server.start()
        server.provision()
        client = sign_in(server.base, "alice", "alicepw")
        for n in range(40):
            client.status_post(f"Repeatable benchmark post {n}: hello #mastomini")
        for workers in (1, 2, 4):
            for mode in ("new", "reuse"):
                for path in bench.DEFAULT_PATHS:
                    rows = bench.batch(server.base, path, workers, args.rounds, mode,
                                       client.access_token, True, 10)
                    summary = bench.summarize(rows)
                    failed |= summary["successful"] != summary["samples"]
                    output["results"].append({"path": path, "concurrency": workers,
                                              "mode": mode, "summary": summary, "samples": rows})
                    print(json.dumps({"path": path, "concurrency": workers, "mode": mode, **summary}), flush=True)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    finally:
        server.close()
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())

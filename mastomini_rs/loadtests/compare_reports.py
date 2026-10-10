"""Side-by-side tables of saved benchmark reports.

    uv run --project loadtests python loadtests/compare_reports.py \\
        --transport loadtests/baselines/*.json --sweep ../.local/locust-*.json

--transport takes transport.py reports (make bench-transport): cold page
views (failures, median) and warm throughput, p95 and connection failures
at each concurrency level, per protocol. --sweep takes sweep.py reports:
throughput, latency and failures per user count, and where failures start.
Each file can be given a label as LABEL=PATH; otherwise its file name is used.
"""
import argparse
import glob
import json
from pathlib import Path


def labelled(items):
    out = []
    for item in items:
        label, _, path = item.rpartition("=") if "=" in item else ("", "", item)
        for match in sorted(glob.glob(path)) or [path]:
            out.append((label or Path(match).stem.replace("transport-", ""), Path(match)))
    return out


def transport_table(reports):
    rows = []
    for label, path in reports:
        d = json.loads(path.read_text())
        health = d.get("health_end") or {}
        heap_min = (health.get("metrics") or {}).get("heap_internal_min")
        for mode in ("h1", "h2"):
            m = d.get("modes", {}).get(mode)
            if not m or "cold" not in m or not m.get("warm"):
                continue  # e.g. no HTTP/2 offered by that firmware
            cold = m["cold"]
            warm = {w["level"]: w for w in m["warm"]}
            top = warm[max(warm)]
            rows.append([
                label, mode,
                f"{cold['failed_views']}/{len(cold['views']) if isinstance(cold['views'], list) else cold['views']}",
                f"{cold['page_ms_median']:.0f}" if cold.get("page_ms_median") else "-",
                "/".join(f"{warm[k]['requests_per_s']:.1f}" for k in sorted(warm)),
                f"{top['p95_ms']:.0f}",
                str(sum(w["connection_failures"] for w in warm.values())),
                f"{heap_min:.0f}" if heap_min is not None else "-",
            ])
    header = ["report", "proto", "cold failed", "cold median ms", "warm req/s (1/3/6)",
              "p95 ms @6", "conn failures", "heap min B"]
    print_table(header, rows)


def sweep_table(reports):
    rows = []
    for label, path in reports:
        d = json.loads(path.read_text())
        stages = d.get("stages", [])
        for s in stages:  # older sweeps lack the derived field
            s.setdefault("successful_requests_per_second",
                         s["requests_per_second"] * (1 - s["failures"] / max(1, s["requests"])))
        clean = [s for s in stages if s["failures"] == 0]
        first_fail = next((s for s in stages if s["failures"]), None)
        best = max(stages, key=lambda s: s["successful_requests_per_second"], default=None)
        at8 = next((s for s in stages if s["users"] == 8), None)
        rows.append([
            label,
            str(max((s["users"] for s in clean), default=0)),
            f"{max((s['requests_per_second'] for s in clean), default=0):.1f}",
            f"{at8['requests_per_second']:.1f} / p50 {at8['p50_ms']} / p95 {at8['p95_ms']}" if at8 else "-",
            (f"{first_fail['users']} users: {first_fail['failures']}/{first_fail['requests']}"
             if first_fail else "none"),
            f"{best['successful_requests_per_second']:.1f} @{best['users']}" if best else "-",
        ])
    header = ["sweep", "max clean users", "max clean req/s", "@8 users: req/s / ms",
              "first failures", "peak successful req/s"]
    print_table(header, rows)


def print_table(header, rows):
    widths = [max(len(str(r[i])) for r in [header] + rows) for i in range(len(header))]
    line = lambda r: "| " + " | ".join(str(c).ljust(w) for c, w in zip(r, widths)) + " |"
    print(line(header))
    print("|" + "|".join("-" * (w + 2) for w in widths) + "|")
    for r in rows:
        print(line(r))
    print()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--transport", nargs="*", default=[], help="transport.py reports ([LABEL=]PATH)")
    parser.add_argument("--sweep", nargs="*", default=[], help="sweep.py reports ([LABEL=]PATH)")
    args = parser.parse_args()
    if args.transport:
        transport_table(labelled(args.transport))
    if args.sweep:
        sweep_table(labelled(args.sweep))


if __name__ == "__main__":
    main()

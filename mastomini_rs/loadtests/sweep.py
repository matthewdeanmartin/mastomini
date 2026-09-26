"""Bounded concurrency sweep; reports warmup separately and checks recovery."""
from gevent import monkey
monkey.patch_all()

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import time

import gevent
from locust.env import Environment
from locust.clients import HttpSession
from locustfile import ReadOnlyUser, configure


def snapshot(host):
    env = Environment()
    with HttpSession(host, env.events.request, None) as client:
        paths = configure(client)
        values = {}
        for key in ("status", "version"):
            response = client.get("/api/mastomini/v1/" + key)
            response.raise_for_status()
            values[key] = response.json()
        return {key: values["status"].get(key) for key in ("available", "accounts", "statuses")} | {
            "uptime_ms": values["version"]["uptime_ms"],
            "fingerprint": values["version"].get("fingerprint"), "paths": paths}


def stats(entry):
    return dict(requests=entry.num_requests, failures=entry.num_failures,
                mean_ms=entry.avg_response_time, p50_ms=entry.get_response_time_percentile(.5),
                p95_ms=entry.get_response_time_percentile(.95),
                p99_ms=entry.get_response_time_percentile(.99), max_ms=entry.max_response_time)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True)
    parser.add_argument("--levels", default="1,2,4,6,8,10,12,16")
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    levels = [int(value) for value in args.levels.split(",")]
    if not levels or any(n < 1 or n > 64 for n in levels) or args.seconds < 5 or args.warmup < 1:
        parser.error("Use 1–64 users, at least 5 measured seconds and 1 warmup second")
    report = dict(started=datetime.now(timezone.utc).isoformat(), host=args.host,
                  seconds=args.seconds, warmup_seconds=args.warmup, before=snapshot(args.host), stages=[])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    previous = report["before"]
    for users in levels:
        env = Environment(user_classes=[ReadOnlyUser], host=args.host, stop_timeout=12)
        runner = env.create_local_runner()
        runner.start(users, spawn_rate=1)
        gevent.sleep(users + args.warmup)
        startup = stats(env.stats.total)
        env.stats.reset_all()
        started = time.monotonic()
        gevent.sleep(args.seconds)
        stage = dict(users=users, warmup=startup, **stats(env.stats.total))
        stage["requests_per_second"] = stage["requests"] / (time.monotonic() - started)
        stage["successful_requests_per_second"] = stage["requests_per_second"] * (1 - stage["failures"] / max(1, stage["requests"]))
        stage["endpoints"] = {name: stats(entry) for (name, method), entry in env.stats.entries.items()}
        runner.quit()
        gevent.sleep(3)
        try:
            stage["after"] = snapshot(args.host)
            stage["recovered"] = bool(stage["after"]["available"])
            stage["counts_unchanged"] = all(stage["after"][key] == report["before"][key] for key in ("accounts", "statuses"))
            stage["no_reboot"] = stage["after"]["uptime_ms"] >= previous["uptime_ms"]
            previous = stage["after"]
        except Exception as exc:
            stage["recovered"] = False
            stage["recovery_error"] = type(exc).__name__
        report["stages"].append(stage)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({key: value for key, value in stage.items() if key != "endpoints"}), flush=True)
        if not stage["recovered"] or not stage.get("counts_unchanged") or not stage.get("no_reboot") or stage["failures"] / max(1, stage["requests"]) > .10:
            break


if __name__ == "__main__":
    main()

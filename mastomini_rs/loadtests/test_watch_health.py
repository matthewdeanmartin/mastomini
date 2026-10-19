import subprocess

from watch_health import counter_delta, dns_probe, safe_sys


def test_counter_delta_does_not_invent_failures_across_reboots():
    before = {"boot_id": 1, "counters": [{"kind": "peer_reset", "count": 40}]}
    after = {"boot_id": 1, "counters": [{"kind": "peer_reset", "count": 43}]}
    assert counter_delta(before, after) == {"peer_reset": 3}
    assert counter_delta(after, after) == {}
    assert counter_delta(None, after) is None
    assert counter_delta(before, {**after, "boot_id": 2}) is None
    assert counter_delta(before, None) is None


def test_sys_snapshot_omits_wifi_identity():
    result = safe_sys({"wifi": {"ssid": "private", "mac": "private", "ip": "private",
                                "rssi": -51, "disconnects": 0}, "future_secret": "private"})
    assert result["wifi"] == {"rssi": -51, "disconnects": 0, "last_reason": None}
    assert "private" not in repr(result)


def test_dns_deadline_is_reported_without_exception_details(monkeypatch):
    def timeout(*args, **kwargs):
        assert kwargs["timeout"] == 3
        raise subprocess.TimeoutExpired("private command", 3)
    monkeypatch.setattr(subprocess, "run", timeout)
    assert dns_probe("mastomini.local", 3)["error"] == "lookup_timeout"


def test_dns_success_retains_addresses_for_duplicate_name_detection(monkeypatch):
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: subprocess.CompletedProcess(
        [], 0, stdout='["192.168.1.161"]'))
    assert dns_probe("mastomini.local", 3)["addresses"] == ["192.168.1.161"]

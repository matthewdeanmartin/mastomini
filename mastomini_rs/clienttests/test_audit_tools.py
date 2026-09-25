"""Regressions for coverage drift detection and benchmark accounting."""

import importlib.util
from pathlib import Path

import pytest


def load(name: str):
    path = Path(__file__).resolve().parents[1] / "scripts" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_coverage_detects_source_changes_and_unclassified_endpoints(tmp_path):
    tool = load("api-coverage")
    (tmp_path / "route.rs").write_text("router")
    data = {"endpoints": [{"method": "GET", "path": "/api/v1/example", "tier": 2,
                           "status": "stub", "note": "Empty", "source": "route.rs"}],
            "reference_endpoints": ["GET /api/v1/example", "POST /api/v1/example"],
            "reviewed_sources": {"route.rs": "old"}}
    errors = tool.validate(data, {"route.rs": "changed", "new.rs": "new"}, tmp_path)
    assert "Review changed source: route.rs" in errors
    assert "Review changed source: new.rs" in errors
    assert "Unclassified reference endpoint: POST /api/v1/example" in errors
    data["endpoints"].append(data["endpoints"][0])
    assert "Duplicate: GET /api/v1/example" in tool.validate(data, {}, tmp_path)


def test_reference_compact_path_expansion():
    tool = load("api-coverage")
    assert tool.expand("/admin/blocks[/{id}]") == ["/admin/blocks", "/admin/blocks/{id}"]
    assert tool.expand("/trends/{tags,links}/{id}/{approve,reject}") == [
        "/trends/tags/{id}/approve", "/trends/tags/{id}/reject",
        "/trends/links/{id}/approve", "/trends/links/{id}/reject"]


def test_benchmark_does_not_count_fast_errors_as_fast_successes():
    tool = load("bench-api")
    summary = tool.summarize([{"status": 200, "ms": 100}, {"status": 401, "ms": 1},
                              {"status": "ReadTimeout", "ms": 1000}])
    assert summary["success_p50_ms"] == 100
    assert summary["successful"] == 1
    assert summary["all_max_ms"] == 1000
    assert tool.summarize([{"status": 404, "ms": 1}])["success_p50_ms"] is None


@pytest.mark.parametrize("path", ["https://other/api/v1/instance", "//other/api/v1/instance", "/oauth/authorize", "/api/v1/instance#fragment"])
def test_benchmark_rejects_cross_origin_and_non_api_paths(path):
    import argparse
    with pytest.raises(argparse.ArgumentTypeError):
        load("bench-api").endpoint(path)


def test_benchmark_real_server_and_error_accounting(server):
    tool = load("bench-api")
    for mode in ("new", "reuse"):
        rows = tool.batch(server.base, "/api/v2/instance", 2, 3, mode, "", True, 5)
        assert len(rows) == 6
        assert all(r["status"] == 200 and r["bytes"] > 0 for r in rows)
        assert sum(r["phase"] == "first" for r in rows) == 2
    rows = tool.batch(server.base, "/api/v1/accounts/verify_credentials", 1, 2, "reuse", "", True, 5)
    assert tool.summarize(rows)["statuses"] == {"401": 2}


def test_tags_notes_and_deferred_policy(household):
    """Check real features and the remaining policy gap through the dispatcher."""
    import requests

    headers = {"Authorization": f"Bearer {household.alice.access_token}"}
    base = household.server.base
    account_id = household.alice.account_verify_credentials().id
    for path in ("/api/v1/featured_tags", "/api/v1/featured_tags/suggestions",
                 "/api/v1/followed_tags", f"/api/v1/accounts/{account_id}/featured_tags"):
        response = requests.get(base + path, headers=headers, timeout=5)
        assert response.status_code == 200
        assert response.json() == []
    for method, path in (("POST", "/api/v1/featured_tags"),
                         ("POST", "/api/v1/tags/python/follow"),
                         ("POST", f"/api/v1/accounts/{account_id}/note")):
        response = requests.request(method, base + path, headers=headers,
                                    json={"name": "python", "comment": "private note"}, timeout=5)
        assert response.status_code == 200, response.text
    household.server.restart()
    response = requests.get(base + "/api/v1/followed_tags", headers=headers, timeout=5)
    assert response.json()[0]["name"] == "python"
    response = requests.get(base + "/api/v1/accounts/relationships", headers=headers,
                            params={"id[]": account_id}, timeout=5)
    assert response.json()[0]["note"] == "private note"
    response = requests.patch(base + "/api/v2/notifications/policy", headers=headers,
                              json={"for_not_followers": "drop"}, timeout=5)
    assert response.status_code == 404
    response = requests.put(base + "/api/v2/notifications/policy", headers=headers,
                            json={"for_not_followers": "drop"}, timeout=5)
    assert response.status_code == 200
    assert response.json()["for_not_followers"] == "accept"

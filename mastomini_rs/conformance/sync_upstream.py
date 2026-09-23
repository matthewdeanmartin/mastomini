"""Copy mastodon_mock's black-box contract tests into tests/, verbatim.

The copies are never edited by hand: anything mastomini does differently is
recorded in deviations.py, so re-running this script picks up upstream
improvements without merge work. Tests that drive mastodon_mock internals
(MockServer seeds, fault injection, the CLI, the UI, alembic) are not copied.

    uv run --project conformance python conformance/sync_upstream.py [MOCK_REPO]
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_REPO = HERE.parents[2] / "mimb" / "mastodon_mock"

# Relative to the upstream tests/ directory. Everything here talks to a server
# only over HTTP, through the fixtures conftest.py provides.
FILES = [
    "test_bughunt_bulk_by_id.py",
    "test_bughunt_grouped_notifications.py",
    "test_bughunt_mastodon_py_contract.py",
    "test_contract_accounts_extra.py",
    "test_contract_admin.py",
    "test_contract_admin_extra.py",
    "test_contract_core.py",
    "test_contract_directory.py",
    "test_contract_discovery.py",
    "test_contract_extended.py",
    "test_contract_filters.py",
    "test_contract_gaps.py",
    "test_contract_grouped_notifications.py",
    "test_contract_lists.py",
    "test_contract_media.py",
    "test_contract_oauth.py",
    "test_contract_openapi_backlog.py",
    "test_contract_pagination.py",
    "test_contract_quotes.py",
    "test_contract_scheduled.py",
    "test_contract_status_validation.py",
    "test_contract_tags_quotes.py",
    "test_contract_timelines.py",
    "test_last_mile_phase_1_2.py",
    "integration/test_integration_readonly.py",
]


def main() -> int:
    repo = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_REPO
    source = repo / "tests"
    if not source.is_dir():
        print(f"{source} not found; pass the mastodon_mock checkout as an argument")
        return 1
    target = HERE / "tests"
    if target.exists():
        shutil.rmtree(target)
    target.mkdir()
    (target / "__init__.py").write_text("")
    for name in FILES:
        dest = target / Path(name).name
        shutil.copyfile(source / name, dest)
    commit = subprocess.run(
        ["git", "-C", str(repo), "log", "-1", "--format=%H %cs"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.strip()
    (HERE / "UPSTREAM.txt").write_text(
        f"mastodon_mock tests copied verbatim from commit {commit or 'unknown'}\n"
        f"Files: {len(FILES)}. Re-sync with sync_upstream.py; never edit tests/ by hand.\n"
    )
    print(f"copied {len(FILES)} files from {repo} ({commit})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

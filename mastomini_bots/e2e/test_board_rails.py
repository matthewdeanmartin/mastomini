"""The deploy scripts' safety rails, without a board: the layouts they check
are the real partition tables, and a mastomini board is always refused."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

CRATE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(CRATE / "scripts"))
import board  # noqa: E402

KINDS = {"app": 0, "data": 1}
SUBTYPES = {"factory": 0x00, "nvs": 0x02, "phy": 0x01, "coredump": 0x03, "spiffs": 0x82}


def layout(csv: Path) -> dict:
    """partitions.csv as the (type, subtype, offset, size) the scripts compare."""
    rows = {}
    for line in csv.read_text().splitlines():
        line = line.split("#")[0].strip()
        if not line:
            continue
        name, kind, subtype, offset, size = [f.strip() for f in line.split(",")[:5]]
        rows[name] = (KINDS[kind], SUBTYPES[subtype], int(offset, 16), int(size, 16))
    return rows


def test_layouts_match_the_partition_tables():
    assert board.BOTS_LAYOUT == layout(CRATE / "partitions.csv")
    assert board.MASTOMINI_LAYOUT == layout(CRATE.parent / "mastomini_rs" / "partitions.csv")
    assert board.BOTS_LAYOUT != board.MASTOMINI_LAYOUT


def test_a_mastomini_board_is_always_refused():
    with pytest.raises(SystemExit, match="runs mastomini"):
        board.refuse_mastomini("11:22:33:44:55:66", board.MASTOMINI_LAYOUT)
    # Known by its MAC even if its table could not be read.
    with pytest.raises(SystemExit, match="runs mastomini"):
        board.refuse_mastomini("ac:a7:04:2c:29:9c", {})
    board.refuse_mastomini("11:22:33:44:55:66", board.BOTS_LAYOUT)
    board.refuse_mastomini("11:22:33:44:55:66", {})


def test_the_boot_log_summary_patterns_match_the_firmware():
    source = (CRATE / "src" / "bin" / "esp32.rs").read_text()
    net = (CRATE / "src" / "bin" / "esp32" / "net.rs").read_text()
    assert 'log::info!("Ready at https://{HOSTNAME}.local/app/ (https://{ip}/app/)")' in source
    assert "No Wi-Fi in this build" in source
    assert 'log::warn!("Could not join {ssid} ({e}); trying again in {pause} s")' in net
    spec = importlib.util.spec_from_file_location("boot_log", CRATE / "scripts" / "boot-log.py")
    assert spec is not None

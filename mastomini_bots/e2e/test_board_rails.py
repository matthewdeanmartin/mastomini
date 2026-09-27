"""The deploy scripts' safety rails, without a board: the layouts they check
are the real partition tables, and a mastomini board is always refused."""

from __future__ import annotations

import importlib.util
import struct
import sys
from pathlib import Path
from types import SimpleNamespace

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


@pytest.mark.parametrize("dry_run", [True, False])
def test_first_install_keeps_factory_usb_board_in_bootloader(tmp_path, monkeypatch, dry_run):
    """A watchdog reset boots factory USB CDC and removes the bootloader port."""
    spec = importlib.util.spec_from_file_location("installer", CRATE / "scripts" / "install.py")
    installer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(installer)
    for name in (board.IMAGE, board.TABLE, board.BOOTLOADER):
        (tmp_path / name).write_bytes(b"\xe9" * 32)
    args = ["install.py", "--port", "COM15", "--images", str(tmp_path),
            "--backup-dir", str(tmp_path / "backups")]
    args += ["--dry-run"] if dry_run else ["--confirm-mac", "ac:a7:04:2c:38:b8"]
    monkeypatch.setattr(sys, "argv", args)
    connected = True
    installed = False
    backed_up = False
    operations = []

    def run(command, **kwargs):
        nonlocal connected, installed, backed_up
        assert connected, "factory firmware removed the bootloader COM port"
        operation = next(x for x in command if x in
                         ("read_mac", "read_flash", "erase_region", "write_flash"))
        operations.append(operation)
        if operation == "read_flash":
            if command[command.index(operation) + 1] == "0":
                Path(command[-1]).write_bytes(b"\xff" * board.FLASH_SIZE)
                backed_up = True
            else:
                table = b""
                if installed:
                    table = b"".join(struct.pack("<HBBII16sI", 0x50AA, kind, subtype,
                                                offset, size, name.encode(), 0)
                                     for name, (kind, subtype, offset, size)
                                     in board.BOTS_LAYOUT.items())
                Path(command[-1]).write_bytes(table.ljust(0x1000, b"\xff"))
        elif operation in ("erase_region", "write_flash"):
            assert backed_up, "no writes are allowed before the full backup"
            assert not dry_run
            if operation == "write_flash":
                installed = True
        if command[command.index("--after") + 1] == "watchdog_reset":
            assert installed and operation == "read_flash", "reset only after final verification"
            connected = False
        return SimpleNamespace(stdout="MAC: ac:a7:04:2c:38:b8\n")

    monkeypatch.setattr(installer.subprocess, "run", run)
    installer.main()
    if dry_run:
        assert operations == ["read_mac", "read_flash"]
        assert connected
    else:
        assert operations == ["read_mac", "read_flash", "read_flash",
                              "erase_region", "erase_region", "erase_region",
                              "write_flash", "read_flash"]
        assert not connected

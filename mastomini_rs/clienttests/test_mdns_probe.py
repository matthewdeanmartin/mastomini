"""The diagnostic parser must reject malformed/unrelated multicast packets."""
import importlib.util
import socket
import struct
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "probe_mdns", Path(__file__).resolve().parents[1] / "scripts" / "probe-mdns.py")
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


def packet(ttl=120, name=b'\x09mastomini\x05local\0'):
    question = name + struct.pack('!HH', 1, 1)
    answer = b'\xc0\x0c' + struct.pack('!HHIH', 1, 0x8001, ttl, 4)
    return struct.pack('!6H', 0x4d4d, 0x8400, 1, 1, 0, 0) + question + answer + socket.inet_aton('192.168.1.161')


def test_compressed_answer_and_cache_flush_bit():
    assert PROBE.addresses(packet(), 'mastomini.local') == ['192.168.1.161']
    assert PROBE.addresses(packet(), 'other.local') == []
    assert PROBE.addresses(packet(ttl=0), 'mastomini.local') == []


def test_truncation_and_pointer_cycles_cannot_hang_the_probe():
    valid = packet()
    for end in range(len(valid)):
        assert PROBE.addresses(valid[:end], 'mastomini.local') == []
    cyclic = struct.pack('!6H', 0, 0x8400, 1, 0, 0, 0) + b'\xc0\x0c\0\1\0\1'
    assert PROBE.addresses(cyclic, 'mastomini.local') == []

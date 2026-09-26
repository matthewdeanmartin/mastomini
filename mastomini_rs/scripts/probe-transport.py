"""Read-only board checks for persistent framing, uploads and static assets.

Uses only GET/HEAD, including deliberately malformed GET framing and a large
GET body that the API must reject. Never authenticates or writes household data.
"""
import argparse
from contextlib import ExitStack
import gzip
import json
from pathlib import Path
import re
import socket
import ssl
import statistics
import time

ROOT = Path(__file__).resolve().parents[1]


class Connection:
    def __init__(self, address, context, secure=True):
        raw = socket.create_connection((address, 443 if secure else 80), timeout=10)
        raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        try:
            self.sock = context.wrap_socket(raw, server_hostname='mastomini.local') if secure else raw
        except BaseException:
            raw.close()
            raise
        self.file = self.sock.makefile('rb')

    def close(self):
        self.file.close()
        self.sock.close()

    def read(self, head=False):
        line = self.file.readline(4096)
        assert line.startswith(b'HTTP/1.1 '), line[:80]
        status = int(line.split()[1])
        headers = {}
        for _ in range(64):
            line = self.file.readline(4096)
            if line == b'\r\n':
                break
            assert line and line.endswith(b'\r\n')
            name, value = line.decode('ascii').split(':', 1)
            headers[name.lower()] = value.strip()
        else:
            raise AssertionError('Too many headers')
        assert 'transfer-encoding' not in headers
        length = int(headers.get('content-length', '0'))
        assert 0 <= length <= 1024 * 1024
        if head or status in (100, 204, 304):
            length = 0
        body = self.file.read(length)
        assert len(body) == length
        return status, headers, body

    def get(self, path='/api/v2/instance', extra='', method='GET'):
        self.sock.sendall(f'{method} {path} HTTP/1.1\r\nHost: mastomini.local\r\n{extra}\r\n'.encode())
        return self.read(head=method == 'HEAD')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--address', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    context = ssl.create_default_context(cafile=str(ROOT / 'certs/household-ca.crt'))
    report = {}
    with ExitStack() as resources:
        def connect(secure=True):
            connection = Connection(args.address, context, secure)
            resources.callback(connection.close)
            return connection
        # Keep old sessions open on purpose: rapid new clients must reclaim
        # idle slots, not fail just because the idle sessions are <1s old.
        for _ in range(12):
            assert connect(secure=False).get()[0] == 200
        report['rapid_http_idle_reclamation'] = 'passed'
        hot = connect()
        status, _, body = hot.get('/api/mastomini/v1/version')
        assert status == 200
        report['before'] = json.loads(body)
        status, headers, body = hot.get(method='HEAD')
        assert status == 200 and int(headers['content-length']) > 0 and not body
        assert hot.get()[0] == 200
        report['head_then_get'] = 'passed'

        first = b'GET /api/v2/instance HTTP/1.1\r\nHost: mastomini.local\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}'
        second = b'GET /api/v1/instance/rules HTTP/1.1\r\nHost: mastomini.local\r\n\r\n'
        hot.sock.sendall(first + second)
        assert hot.read()[0] == 200
        assert hot.read()[0] == 200
        report['pipelined_get_body'] = 'passed'

        slow = connect()
        slow.sock.sendall(b'GET /api/v2/instance HTTP/1.1\r\nHost: ')
        times = []
        for _ in range(20):
            started = time.perf_counter()
            assert hot.get()[0] == 200
            times.append((time.perf_counter()-started)*1000)
        report['hot_during_partial_request'] = dict(mean_ms=statistics.mean(times), max_ms=max(times))

        # Exercise the full 160 KiB transport upload bound without making a
        # mutation: this GET route has the normal 4 KiB API body limit (413).
        hot.sock.sendall(b'GET /api/v2/instance HTTP/1.1\r\nHost: mastomini.local\r\nContent-Length: 163840\r\nExpect: 100-continue\r\n\r\n')
        assert hot.read()[0] == 100
        for _ in range(40):
            hot.sock.sendall(b'x' * 4096)
        assert hot.read()[0] == 413
        assert hot.get()[0] == 200
        report['continue_large_body_then_reuse'] = 'passed'

        bad = connect()
        bad.sock.sendall(b'GET /api/v2/instance HTTP/1.1\r\nHost: mastomini.local\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n')
        status, headers, _ = bad.read()
        assert status == 400 and headers.get('connection') == 'close'
        assert bad.file.read(1) == b''
        report['ambiguous_framing_rejected'] = 'passed'

        status, headers, body = hot.get('/app/', 'Accept-Encoding: gzip\r\n')
        assert status == 200 and headers['content-encoding'] == 'gzip'
        index = gzip.decompress(body)
        asset = re.search(rb'src="([A-Za-z0-9_-]+\.js)"', index).group(1).decode()
        status, headers, body = hot.get('/app/' + asset, 'Accept-Encoding: gzip\r\n')
        local = ROOT.parent / 'mastomini_ui/dist/mastomini-ui/browser' / asset
        assert status == 200 and gzip.decompress(body) == local.read_bytes()
        report['exact_large_asset'] = dict(compressed_bytes=len(body))
        status, _, body = hot.get('/app/' + asset,
            f'Accept-Encoding: gzip\r\nIf-None-Match: {headers["etag"]}\r\n')
        assert status == 304 and not body
        assert hot.get()[0] == 200
        report['not_modified_then_reuse'] = 'passed'
        status, _, body = hot.get('/api/mastomini/v1/version')
        assert status == 200
        report['after'] = json.loads(body)
        assert report['after']['uptime_ms'] >= report['before']['uptime_ms']
    args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()

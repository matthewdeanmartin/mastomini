"""Read a Firefox networking log (about:logging, preset "Networking") and say
why the board's connections closed and which requests that took down.

    uv run --project loadtests python loadtests/firefox_log.py C:/path/to/firefox.moz_log
    uv run --project loadtests python loadtests/firefox_log.py log.moz_log --host mastomini.local --timeline

To capture: about:logging -> Logging presets: Networking -> Save to a file
-> Start logging, reproduce, Stop logging. The file is large (100+ MB);
it is read line by line.

What it reports, for connections to --host only:
  - HTTP/2 sessions Firefox closed, by reason (0 = clean);
  - "poison" activations: a request whose activation failed and made
    Firefox close the whole connection (804b005d was Local Network Access:
    the request's permission was still pending);
  - streams aborted with those connections (80004004): these reach the page
    as "CORS request did not succeed" and are NOT retried by Firefox;
  - Local Network Access permission grants, and GOAWAYs in both directions.

It found the October 2026 Mawkingbird failures: Firefox 157's Local Network
Access, fixed per browser with network.lna.skip-domains (spec/09).
Nothing from the log is echoed except redacted URLs.
"""
import argparse
import collections
import re
import sys
from pathlib import Path

from diag_common import redact

LINE = re.compile(r"^(?:\d{4}-\d\d-\d\d )?(\d\d:\d\d:\d\d\.\d+) UTC - \[([^\]]*)\]: \w/(\w+) (.*)$")
# Firefox nsresult codes seen in these logs (hex, as Firefox prints them).
REASONS = {
    "0": "clean (NS_OK)",
    "80004004": "aborted (NS_BINDING_ABORTED)",
    "804b0014": "connection reset (NS_ERROR_NET_RESET)",
    "804b0047": "connection interrupted (NS_ERROR_NET_INTERRUPT)",
    "804b000e": "timed out (NS_ERROR_NET_TIMEOUT)",
    "804b000d": "connection refused (NS_ERROR_CONNECTION_REFUSED)",
    "804b005d": "activation refused; seen with a pending Local Network Access permission",
    "80470002": "stream closed (NS_BASE_STREAM_CLOSED)",
}


def reason(code: str) -> str:
    code = code.lower().lstrip("0x") or "0"
    return f"{code} {REASONS.get(code, '')}".strip()


class Analysis:
    def __init__(self, host: str):
        self.host = host
        self.conn_host = {}        # nsHttpConnection -> host
        self.thread_conn = {}      # thread -> connection that just started HTTP/2
        self.session_conn = {}     # Http2Session -> nsHttpConnection
        self.closes = []           # (time, session, reason)
        self.poison = []           # (time, connection, transaction, reason)
        self.aborted = []          # (time, session, stream id)
        self.goaway_sent = []      # (time, session, code)
        self.goaway_recv = []      # (time, session, code)
        self.lna = []              # time of each granted permission
        self.lna_other = collections.Counter()
        self.urls = {}             # Http2Stream pointer -> URI, for --timeline
        self.first = self.last = None

    def ours(self, session: str) -> bool:
        conn = self.session_conn.get(session)
        return conn is not None and self.host in self.conn_host.get(conn, "")

    def feed(self, line: str) -> None:
        m = LINE.match(line)
        if not m:
            return
        t, thread, _module, text = m.groups()
        self.first = self.first or t
        self.last = t
        if (c := re.search(r"nsHttpConnection::OnSocketWritable \[this=(\w+)\] host=(\S+)", text)):
            self.conn_host[c[1]] = c[2]
        elif (c := re.search(r"nsHttpConnection::StartSpdy \[this=(\w+)", text)):
            self.thread_conn[thread] = c[1]
        elif (c := re.match(r"Http2Session::Http2Session (\w+)", text)):
            if thread in self.thread_conn:
                self.session_conn[c[1]] = self.thread_conn.pop(thread)
        elif (c := re.search(r"nsHttpConnection::DontReuse (\w+) spdysession=(\w+)", text)):
            self.session_conn.setdefault(c[2], c[1])
        elif (c := re.match(r"Http2Session::Close (\w+) (\w+)", text)):
            self.closes.append((t, c[1], c[2]))
        elif (c := re.search(r"nsHttpConnection::CloseTransaction\s*\[this=(\w+) trans=(\w+) reason=(\w+)\]", text)):
            if c[3].lower() not in ("0", "80004004"):
                self.poison.append((t, c[1], c[2], c[3]))
        elif (c := re.match(r"Http2Session::CloseStream (\w+) \w+ (0x\w+) 80004004", text)):
            self.aborted.append((t, c[1], c[2]))
        elif (c := re.match(r"Http2Session::GenerateGoAway (\w+) code=(\w+)", text)):
            self.goaway_sent.append((t, c[1], c[2]))
        elif (c := re.match(r"Http2Session::RecvGoAway (\w+) GOAWAY Last-Good-ID \w+ status (\w+)", text)):
            self.goaway_recv.append((t, c[1], c[2]))
        elif "LNAPermissionRequest" in text:
            if "granted" in text:
                self.lna.append(t)
            else:
                self.lna_other[text.split("LNAPermissionRequest", 1)[1].strip()[:40]] += 1
        elif (c := re.match(r"Http2Stream (\w+) Stream ID 0x\w+ \[session=\w+\] for URI (\S+)", text)):
            self.urls[c[1]] = c[2]

    def report(self, timeline: bool) -> int:
        print(f"log {self.first} .. {self.last} UTC; host filter {self.host!r}")
        ours = [c for c in self.closes if self.ours(c[1])]
        by_reason = collections.Counter(reason(code) for _, _, code in ours)
        print(f"\nHTTP/2 sessions to the board closed by Firefox: {len(ours)}")
        for what, n in by_reason.most_common():
            print(f"  {n:4}  {what}")
        poison = [p for p in self.poison if self.host in self.conn_host.get(p[1], "")]
        killed = {p[1] for p in poison}
        print(f"\nrequests whose activation killed a connection: {len(poison)} "
              f"({len(killed)} connections)")
        for what, n in collections.Counter(reason(p[3]) for p in poison).most_common():
            print(f"  {n:4}  {what}")
        bad_sessions = {s for _, s, code in ours if code not in ("0",)}
        collateral = [a for a in self.aborted if a[1] in bad_sessions]
        print(f"streams aborted with those connections (page sees 'CORS request did not succeed'): "
              f"{len(collateral)}")
        print(f"\nLocal Network Access permissions granted: {len(self.lna)}"
              + (f" ({self.lna[0]} .. {self.lna[-1]})" if self.lna else ""))
        for what, n in self.lna_other.most_common():
            print(f"  other LNA results: {n} x {what}")
        sent = [g for g in self.goaway_sent if self.ours(g[1])]
        recv = [g for g in self.goaway_recv if self.ours(g[1])]
        print(f"GOAWAY sent by Firefox: {dict(collections.Counter(c for *_, c in sent))}; "
              f"from the board: {dict(collections.Counter(c for *_, c in recv))}")
        if timeline:
            print("\ntimeline (failures only):")
            events = [(t, f"connection {c} killed by transaction {tr}: {reason(r)}") for t, c, tr, r in poison]
            events += [(t, f"session {s} closed: {reason(r)}") for t, s, r in ours if r != "0"]
            for t, line in sorted(events):
                print(f"  {t}  {line}")
        print("\nverdict:", self.verdict(poison, ours, recv))
        return 1 if poison or bad_sessions else 0

    def verdict(self, poison, ours, recv) -> str:
        if any(p[3].lower() == "804b005d" for p in poison) and self.lna:
            return ("Firefox's Local Network Access killed shared HTTP/2 connections while permissions "
                    "were pending. Per browser: about:config network.lna.skip-domains = the board's "
                    "name (spec/09). Not a board fault.")
        if any(code not in ("0", "00000000") for *_, code in recv):
            return "the board sent GOAWAY with an error: check /.well-known/incidents (h2_goaway)."
        if poison or any(code != "0" for *_, code in ours):
            return "Firefox closed connections with errors; see the reasons above."
        return "no connection-level failures to the board in this log."


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("log", type=Path)
    parser.add_argument("--host", default="mastomini.local", help="the board's name as Firefox used it")
    parser.add_argument("--timeline", action="store_true", help="list each failure with its time")
    args = parser.parse_args()
    analysis = Analysis(args.host)
    with args.log.open(encoding="utf-8", errors="replace") as f:
        for line in f:
            analysis.feed(redact(line.rstrip("\n")) if "token" in line else line.rstrip("\n"))
    sys.exit(analysis.report(args.timeline))


if __name__ == "__main__":
    main()

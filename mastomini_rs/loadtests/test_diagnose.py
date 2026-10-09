"""Unit tests for the connection diagnostics' pure parts (no board needed)."""
import json

from diag_common import percentile, read_token, redact
from firefox_log import Analysis
from har_summary import summarize


def test_redact_hides_tokens_in_urls_and_headers():
    url = "/api/v1/streaming?access_token=3HlxzXOai6fEIeLDM1Z3R9sMx7R&stream=user"
    assert redact(url) == "/api/v1/streaming?access_token=[redacted]&stream=user"
    assert redact("Authorization: Bearer abc.DEF-123") == "Authorization: Bearer [redacted]"
    assert redact("code=xyz&client_secret=s3cret") == "code=[redacted]&client_secret=[redacted]"
    assert redact("/api/v1/instance") == "/api/v1/instance"


def test_read_token_accepts_the_json_file_or_a_bare_token(tmp_path):
    saved = tmp_path / "diag-token"
    saved.write_text(json.dumps({"client_id": "c", "client_secret": "s", "access_token": "T1"}))
    assert read_token(saved) == "T1"
    bare = tmp_path / "bare"
    bare.write_text("T2\n")
    assert read_token(bare) == "T2"


def test_percentile_is_nearest_rank():
    assert percentile([], 0.5) is None
    values = sorted([0.2, 0.3, 0.4, 2.6])
    assert percentile(values, 0.5) == 0.4
    assert percentile(values, 0.9) == 2.6


def line(t, text, thread="Parent 1: Socket Thread", level="V"):
    return f"2026-10-09 {t} UTC - [{thread}]: {level}/nsHttp {text}"


# The October 2026 pattern, shortened: a request whose Local Network Access
# permission is still pending kills the shared HTTP/2 connection.
FIREFOX = [
    line("15:06:40.761000", "nsHttpConnection::OnSocketWritable [this=c1] host=mastomini.local"),
    line("15:06:40.762000", "nsHttpConnection::StartSpdy [this=c1, mDid0RTTSpdy=0]"),
    line("15:06:40.762000", "Http2Session::Http2Session s1 serial=0x18A0", level="I"),
    line("15:06:40.763000", "Http2Stream x1 Stream ID 0x3 [session=s1] for URI /api/v2/instance", level="I"),
    line("15:06:40.765000", "nsHttpConnection::CloseTransaction[this=c1 trans=t9 reason=804b005d]"),
    line("15:06:40.765000", "Http2Session::Close s1 804B005D", level="I"),
    line("15:06:40.765000", "Http2Session::CloseStream s1 x1 0x3 80004004", level="I"),
    line("15:06:40.765000", "Http2Session::GenerateGoAway s1 code=2", level="I"),
    line("15:06:42.952000", "nsHttpChannel::OnPermissionPromptResult [this=ch] LNAPermissionRequest granted"),
    # Another site's session, closed cleanly: not counted.
    line("15:06:43.000000", "nsHttpConnection::OnSocketWritable [this=c2] host=example.org"),
    line("15:06:43.001000", "nsHttpConnection::StartSpdy [this=c2, mDid0RTTSpdy=0]"),
    line("15:06:43.002000", "Http2Session::Http2Session s2 serial=0x1", level="I"),
    line("15:06:44.000000", "Http2Session::Close s2 0", level="I"),
]


def test_firefox_log_finds_lna_connection_kills(capsys):
    analysis = Analysis("mastomini.local")
    for text in FIREFOX:
        analysis.feed(text)
    assert analysis.report(timeline=True) == 1
    out = capsys.readouterr().out
    assert "sessions to the board closed by Firefox: 1" in out
    assert "requests whose activation killed a connection: 1 (1 connections)" in out
    assert "streams aborted with those connections (page sees 'CORS request did not succeed'): 1" in out
    assert "Local Network Access permissions granted: 1" in out
    assert "network.lna.skip-domains" in out
    assert "example.org" not in out


def test_firefox_log_without_failures_says_so(capsys):
    analysis = Analysis("mastomini.local")
    for text in FIREFOX[:3] + [line("15:07:00.000000", "Http2Session::Close s1 0", level="I")]:
        analysis.feed(text)
    assert analysis.report(timeline=False) == 0
    assert "no connection-level failures" in capsys.readouterr().out


def entry(at, method, path, status, timings):
    return {"startedDateTime": f"2026-10-09T10:12:{at:06.3f}-04:00", "time": sum(max(v, 0) for v in timings.values()),
            "request": {"method": method, "url": f"https://mastomini.local{path}", "headers": []},
            "response": {"status": status, "httpVersion": "HTTP/2" if status else ""},
            "timings": timings}


def test_har_summary_classifies_failures_and_redacts():
    full = {"blocked": 0, "dns": 0, "connect": 20, "ssl": 982, "send": 0, "wait": 0, "receive": 0}
    none = dict.fromkeys(full, -1)
    ok = {"blocked": 0, "dns": 0, "connect": 0, "ssl": 0, "send": 0, "wait": 150, "receive": 1}
    har = {"log": {"entries": [
        entry(27.0, "OPTIONS", "/api/v1/trends/tags", 0, full),
        entry(27.0, "GET", "/api/v1/trends/tags", 0, none),
        entry(28.0, "GET", "/api/v1/streaming?access_token=SECRET", 404, ok),
        entry(29.0, "GET", "/api/v2/instance", 200, ok),
    ]}}
    s = summarize(har, "mastomini.local")
    assert (s["requests"], s["failed"]) == (4, 2)
    assert s["failed_after_handshake"] == 1
    assert s["failed_never_sent"] == 1
    assert s["new_connections"] == 1
    assert all("SECRET" not in r["path"] for r in s["rows"])

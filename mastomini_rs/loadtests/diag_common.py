"""Shared pieces of the connection diagnostics (h2_burst, transfer_stalls,
idle_close, browser_session, oauth_token, firefox_log, har_summary).

They were written while chasing browser "CORS request did not succeed"
errors (October 2026, DEPLOY.md and spec/09): failures that turned out to be
dropped connections, truncated TLS writes, refused HTTP/2 streams and
Firefox's Local Network Access checks, never a missing CORS header.
"""
import argparse
import json
import re
import socket
import ssl
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CRATE = ROOT.parent
REPORTS = ROOT / "reports"
DEFAULT_CA = CRATE / "certs" / "household-ca.crt"
# Repo-ignored (/.local/): where a diagnostic token is kept between runs.
DEFAULT_TOKEN_FILE = CRATE.parent / ".local" / "diag-token"

_SECRET_PARAM = re.compile(r"((?:access_token|token|code|client_secret|password)=)[^&\s\"']+", re.I)
_BEARER = re.compile(r"(Bearer\s+)[A-Za-z0-9._~+/=-]+", re.I)


def redact(text: str) -> str:
    """Hide tokens, codes and secrets in URLs and headers before printing.

    A HAR or browser log carries the session's token (a streaming URL has it
    in its query string): never echo one into a terminal or a report.
    """
    return _BEARER.sub(r"\1[redacted]", _SECRET_PARAM.sub(r"\1[redacted]", text))


def board_args(parser: argparse.ArgumentParser) -> None:
    """The board's address, TLS name and household CA, as transport.py takes them."""
    parser.add_argument("--address", required=True, help="board IP (mDNS is slow on Windows)")
    parser.add_argument("--host", default="mastomini.local", help="TLS name and Host header")
    parser.add_argument("--ca", type=Path, default=DEFAULT_CA, help="household CA certificate")
    parser.add_argument("--insecure", action="store_true",
                        help="skip certificate checks (a board whose CA is not at hand)")


def tls_context(args, alpn=("h2", "http/1.1")) -> ssl.SSLContext:
    if args.insecure:
        ctx = ssl.create_default_context()
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
    else:
        ctx = ssl.create_default_context(cafile=str(args.ca))
    ctx.set_alpn_protocols(list(alpn))
    return ctx


def connect(args, ctx: ssl.SSLContext, timeout=15.0, session=None) -> ssl.SSLSocket:
    raw = socket.create_connection((args.address, 443), timeout=timeout)
    return ctx.wrap_socket(raw, server_hostname=args.host, session=session)


def percentile(sorted_values, p: float):
    """Nearest-rank percentile of an already sorted list (None when empty)."""
    if not sorted_values:
        return None
    return sorted_values[min(len(sorted_values) - 1, int(p * len(sorted_values)))]


def read_token(path: Path) -> str:
    """The access token from oauth_token.py's JSON file, or a file holding just a token."""
    raw = Path(path).read_text().strip()
    try:
        return json.loads(raw)["access_token"]
    except (ValueError, KeyError, TypeError):
        return raw

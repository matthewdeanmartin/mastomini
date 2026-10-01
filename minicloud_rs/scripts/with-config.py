"""Run a Git Bash command with private firmware settings; never print values."""
import json
import os
from pathlib import Path
import subprocess
import sys

KEYS = ('MINICLOUD_WIFI_SSID', 'MINICLOUD_WIFI_PASSWORD', 'MINICLOUD_ADMIN_TOKEN')
ROOT = Path(__file__).resolve().parents[1]


def settings():
    values = {}
    path = ROOT / '.env'
    if path.exists():
        for line in path.read_text(encoding='utf-8').splitlines():
            line = line.strip().removeprefix('export ')
            if not line or line.startswith('#') or '=' not in line:
                continue
            key, value = line.split('=', 1)
            key, value = key.strip(), value.strip()
            if key not in KEYS:
                continue
            if value.startswith('"'):
                value = json.loads(value)
            elif len(value) >= 2 and value[0] == value[-1] == "'":
                value = value[1:-1]
            else:
                value = value.split('#', 1)[0].strip()
            if not isinstance(value, str):
                raise ValueError(f'{key} must be a string')
            values[key] = value
    for key in KEYS:
        if key in os.environ:
            values[key] = os.environ[key]
        if not values.get(key):
            raise ValueError(f'Set {key} in the environment or private minicloud_rs/.env')
    if len(values['MINICLOUD_WIFI_SSID'].encode()) > 32:
        raise ValueError('SSID exceeds 32 bytes')
    if len(values['MINICLOUD_WIFI_PASSWORD'].encode()) > 63:
        raise ValueError('Wi-Fi password exceeds 63 bytes')
    if len(values['MINICLOUD_ADMIN_TOKEN']) < 24:
        raise ValueError('Management token must contain at least 24 characters')
    return values


if __name__ == '__main__':
    if len(sys.argv) < 2:
        raise SystemExit('Usage: python scripts/with-config.py command [arguments...]')
    try:
        env = dict(os.environ, **settings(), MINICLOUD_CONFIG_LOADED='1')
        raise SystemExit(subprocess.call(sys.argv[1:], env=env, cwd=ROOT))
    except (ValueError, OSError) as error:
        # Parsing errors can contain the original input; keep secrets off logs.
        raise SystemExit(f'Private build configuration could not be loaded ({type(error).__name__})') from None

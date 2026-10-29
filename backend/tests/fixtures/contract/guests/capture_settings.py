"""Captura coerção das opções do hub no Settings original, com dotenv isolado."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch, CP_AUTH_TOKEN='owner')
    from app.config import Settings, _LOOPBACK
    from pydantic import ValidationError
    rows = []
    for field, values in [
        ('sync_rate_max', ['10', '-1', str(10**80), '1.0', '1e3', ' 2 ', '+3', '1_000', 'no']),
        ('sync_rate_window', ['300', '-1', str(10**80), '0', '1.5']),
        ('lan_bind_ip', ['127.0.0.1', '127.0.0.2', 'localhost', '::1', '0.0.0.0', 'auto', '192.0.2.1']),
        ('sync', ['1', 'true', 't', 'y', 'on', 'off', ' True ', '', '2']),
    ]:
        key = 'CP_' + field.upper()
        for value in values:
            dotenv = root / '.env'
            dotenv.write_text(f'{key}={json.dumps(value)}\n')
            try:
                result = getattr(Settings(_env_file=dotenv), field)
                record = {'accepted': True, 'value': result}
                if field == 'lan_bind_ip': record['secure'] = result not in _LOOPBACK
            except ValidationError as exc:
                record = {'accepted': False, 'errors': exc.errors(include_url=False)}
            rows.append({'field': field, 'input': value, **record})
    result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'command': 'cd backend && uv run python tests/fixtures/contract/guests/capture_settings.py', 'rows': rows}
    (HERE / 'settings.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    print(f'{len(rows)} opções capturadas')

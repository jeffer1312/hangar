"""Captura Settings e asyncio.sleep originais com ambiente e dotenv isolados."""
import asyncio
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
for key in list(os.environ):
    if key.upper().startswith('CP_'):
        del os.environ[key]

async def sleep_result(value):
    try:
        await asyncio.wait_for(asyncio.sleep(value), timeout=0.005)
        return 'completed'
    except TimeoutError:
        return 'waiting'
    except Exception as error:
        return type(error).__name__

with tempfile.TemporaryDirectory() as scratch:
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch)
    os.chdir(root)
    from app.config import Settings
    from pydantic import ValidationError
    rows = []
    for field, inputs in [
        ('auto_resume', ['true', 'false', '1', '0', 'yes', 'off', ' True ', '', '2']),
        ('stall_poll_seconds', ['30', '0', '-1', '1.0', '1.5', '1e3', ' 2 ', '+3', '1_000', 'no', str(10**80), str(10**400)]),
    ]:
        for raw in inputs:
            dotenv = root / '.env'
            dotenv.write_text('CP_' + field.upper() + '=' + json.dumps(raw) + '\n')
            try:
                value = getattr(Settings(_env_file=dotenv), field)
                result = {'accepted': True, 'value': value}
                if field == 'stall_poll_seconds':
                    result['sleep'] = asyncio.run(sleep_result(value))
            except ValidationError as error:
                result = {'accepted': False, 'errors': error.errors(include_url=False)}
            rows.append({'field': field, 'input': raw, **result})
    data = {
        'source_commit': subprocess.check_output(['git', '-C', str(HERE), 'rev-parse', 'HEAD'], text=True).strip(),
        'command': 'uv run --directory backend python tests/fixtures/contract/guests/capture_watch_settings.py',
        'isolation': 'Settings original; HOME, USERPROFILE, CLAUDE_CONFIG_DIR, cwd e dotenv temporários; CP_* removidos; sleep limitado por wait_for a 5 ms.',
        'rows': rows,
    }
    (HERE / 'watch-settings.json').write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n')
    print(f'{len(rows)} casos capturados')

"""Executa o capturador original com processos locais descartáveis."""
import json
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from app import config_sync

cases = []
for name, source in [
    ('stdout', 'print("resultado")'),
    ('stderr', 'import sys; print("saída"); print("erro", file=sys.stderr); sys.exit(3)'),
    ('empty', 'pass'),
    ('unicode_tail', 'print("á" * 350)'),
    ('newlines', 'import sys; sys.stdout.buffer.write(b"first\\rsecond\\r\\nthird\\n")'),
    ('decode', 'import sys; sys.stderr.buffer.write(b"bad\\xff")'),
    ('config_env', 'import os; print(os.environ.get("CLAUDE_CONFIG_DIR", "ausente"))'),
]:
    with patch.dict('os.environ', {'CLAUDE_CONFIG_DIR': '/fixture/secondary'}):
        result = config_sync._run([sys.executable, '-c', source])
    cases.append({'name': name, 'source': source, 'result': result})
for name, error in [('missing', FileNotFoundError()), ('permission', PermissionError()),
                    ('timeout', subprocess.TimeoutExpired('fixture', 180))]:
    with patch.object(config_sync.subprocess, 'run', side_effect=error):
        result = config_sync._run(['fixture'])
    cases.append({'name': name, 'result': result})
result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
          'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_commands.py',
          'injections': ['erros de abertura e timeout injetados; demais processos reais'], 'cases': cases}
Path(__file__).with_name('commands.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')

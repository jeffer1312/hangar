"""Contratos executados de validação usados pela importação de preferências."""
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from app import runtime_config

values = [None, True, False, 0, 1, -1, 30, 31, 1.5, '', '  texto  ', '1', '1.5',
          'https://example.invalid/v1/chat/completions?x=1', 'low', 'none', [], {},
          [{'id': 'a', 'kind': 'elevenlabs', 'api_key': 'fixture-key'}],
          [{'id': 'a', 'kind': 'whisper_cpp', 'executable_path': '/opt/whisper', 'model_path': '/opt/model'}]]
cases = []
for key in runtime_config.EDITAVEIS:
    if key in ('sync', 'scan_roots', 'term_origins'):
        continue
    for value in values:
        case = {'key': key, 'input': value}
        try:
            case['output'] = runtime_config._coagir(key, value)
        except Exception as exc:
            case['error'] = str(exc)
            case['error_type'] = type(exc).__name__
        cases.append(case)
result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
          'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_preferences.py',
          'cases': cases}
Path(__file__).with_name('preferences.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')

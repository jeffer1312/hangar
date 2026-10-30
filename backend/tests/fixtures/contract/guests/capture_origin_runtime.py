"""Captura origens do terminal usando o leitor original de runtime-config."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from pytest import MonkeyPatch
from app import runtime_config as rc, termsock

rows = []
for raw in ['{}', '{"term_origins":"https://saved.example"}',
            '{"term_origins":"https://saved.example","extra":NaN}',
            '{"term_origins":"https://saved.example","extra":Infinity}',
            '{"term_origins":"https://saved.example","extra":"\\ud800"}',
            '{"term_origins":null}', '{"term_origins":false}', '{"term_origins":42}',
            '{"term_origins":["https://saved.example"]}', 'invalid']:
    for origin in ['https://extra.example', 'https://saved.example', 'https://other.example']:
        with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp:
            path = Path(folder)
            (path / 'runtime-config.json').write_text(raw)
            settings = SimpleNamespace(term_origins='https://extra.example', public_url='')
            mp.setattr(rc, '_backend_config_base', lambda: path)
            mp.setattr(rc, '_cache', None)
            mp.setattr(rc, 'settings', settings)
            mp.setattr(termsock, 'settings', settings)
            mp.setattr(termsock, '_peers_conhecidos', lambda: [])
            rows.append({'raw': raw, 'origin': origin,
                         'accepted': termsock._origem_aceita(origin, 'fixture')})

Path(__file__).with_name('origin-runtime.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/guests/capture_origin_runtime.py',
    'injections': ['settings congelado', 'peers vazio', 'runtime-config em diretório temporário'],
    'rows': rows,
}, ensure_ascii=True, indent=2) + '\n')

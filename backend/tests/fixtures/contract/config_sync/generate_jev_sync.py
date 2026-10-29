"""Congela a publicação das preferências no MCP ativo e guardado, incluindo falhas."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from pytest import MonkeyPatch
from app import computer_control as cc, runtime_config as rc
from app.config import Settings

old = {'LLM_PROXY_URL': 'https://old.test/v1/chat/completions', 'LLM_MODEL': 'antigo', 'LLM_PROXY_KEY': 'fixture-old', 'KEEP': 'preservar'}
scenarios = [
    ('empty', {}, None, None, [{'jev_aux_modelo': 'novo'}]),
    ('active', {}, {'command': 'fixture', 'env': old}, None, [{'jev_aux_modelo': 'novo'}, {'jev_aux_modelo': 'seguinte'}]),
    ('parked', {}, None, {'command': 'fixture', 'env': old}, [{'jev_aux_api_key': 'fixture-new'}]),
    ('both', {}, {'command': 'fixture', 'env': old}, {'command': 'fixture', 'env': old}, [{'jev_api_key': 'sk-or-fixture'}]),
    ('browser', {'jev_texto_base_url': 'https://browser.test/v1', 'jev_texto_modelo': 'browser', 'jev_texto_api_key': 'fixture-browser'}, None, None, [{'jev_aux_esforco': 'high'}]),
    ('remove', {}, {'command': 'fixture', 'env': old}, None, [{'jev_aux_modelo': None}]),
    ('invalid_active', {}, '{bad', None, [{'jev_aux_modelo': 'novo'}]),
    ('invalid_parked', {}, None, '[]', [{'jev_aux_modelo': 'novo'}]),
    ('bad_env', {'jev_aux_modelo': 'existente'}, {'command': 'fixture', 'env': [1]}, None, [{'jev_aux_modelo': 'novo'}]),
    ('invalid_choice', {}, {'command': 'fixture', 'env': old}, None, [{'jev_aux_esforco': 'inválido'}]),
]
cases = []
for name, initial, active, parked, steps in scenarios:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True):
        home = Path(folder)
        config = home / '.claude'
        config.mkdir()
        (home / '.hangar').mkdir()
        runtime = config / 'runtime-config.json'
        runtime.write_text(json.dumps(initial, ensure_ascii=False))
        for path, value in [(home / '.claude.json', active), (home / '.hangar/computer-control.json', parked)]:
            if value is not None:
                data = {'other': 'preservar', 'mcpServers': {cc.NAME: value}} if path.name == '.claude.json' and isinstance(value, dict) else value
                path.write_text(value if isinstance(value, str) else json.dumps(data))
        mp.setattr(rc, '_backend_config_base', lambda: config)
        mp.setattr(rc, '_cache', None)
        mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
        mp.setattr(cc, '_config_files', lambda: [home / '.claude.json'])
        mp.setattr(cc, '_main_file', lambda: home / '.claude.json')
        mp.setattr(cc, '_parked_file', lambda: home / '.hangar/computer-control.json')
        mp.setattr(cc, '_old_llm_file', lambda: home / '.hangar/computer-use-llm-antigo.json')
        def tree():
            return {str(path.relative_to(home)): path.read_text().replace(folder, '⟦HOME⟧') for path in sorted(home.rglob('*')) if path.is_file()}
        before = tree()
        observations = []
        for body in steps:
            try:
                result = {'ok': rc.aplicar({k:v for k,v in body.items() if v is not None}, remover={k for k,v in body.items() if v is None})}
            except Exception as exc:
                result = {'error': str(exc).replace(folder, '⟦HOME⟧'), 'kind': type(exc).__name__}
            observations.append({'body': body, 'result': result, 'files': tree()})
        cases.append({'name': name, 'files': before, 'steps': observations})
result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
          'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_jev_sync.py', 'cases': cases}
Path(__file__).with_name('jev-sync.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')

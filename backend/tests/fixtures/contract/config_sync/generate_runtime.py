"""Congela leituras e escritas de preferências em uma casa sem credenciais reais."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch as mock_patch

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from pytest import MonkeyPatch
from app import computer_control as cc, pensamento, runtime_config as rc
from app.config import Settings

scenarios = [
    ('default', None, None, [{}]),
    ('unknown', None, None, [{'auth_token': 'fixture', 'port': 1}]),
    ('override_remove', None, None, [{'upload_retention_days': 7}, {'upload_retention_days': None}]),
    ('mask', None, None, [{'groq_api_key': 'gsk_abcdefghijklmnop'}, {'groq_api_key': ' gsk_••••••••mnop '}, {'groq_api_key': ''}, {'groq_api_key': None}]),
    ('keep_unknown', '{"codex_voice_beta": true}', None, [{'claude_statusline_update': False}]),
    ('invalid_type', None, None, [{'upload_retention_days': 'trinta'}]),
    ('thinking', None, '{"model":"opus","hooks":{}}', [{'mostrar_pensamento': True}, {'mostrar_pensamento': True}, {'mostrar_pensamento': False}]),
    ('validate_before_thinking', None, None, [{'mostrar_pensamento': True, 'upload_retention_days': 'bad'}]),
    ('thinking_invalid', None, '{bad', [{'mostrar_pensamento': True, 'editor': 'vim'}]),
    ('thinking_root', None, '[]', [{'mostrar_pensamento': True}]),
    ('thinking_empty', None, '', [{'mostrar_pensamento': True}]),
    ('legacy_key', None, None, [{'jev_windows_api_key': 'nova-chave'}]),
    ('legacy_mask', None, None, [{'jev_windows_api_key': '••••••••'}]),
    ('bad_runtime', '{bad', None, [{'editor': 'vim'}]),
    ('root_runtime', '[]', None, [{'editor': 'vim'}]),
    ('cliproxy_absent', None, None, [{'jev_aux_cliproxy': True}]),
    ('cliproxy_access', None, None, [{'jev_aux_cliproxy': True}]),
    ('cliproxy_false', None, None, [{'jev_aux_cliproxy': False}]),
    ('cliproxy_one', None, None, [{'jev_aux_cliproxy': 1}]),
    ('cliproxy_overrides_body', None, None, [{'jev_aux_cliproxy': True, 'jev_aux_api_key': 'body-key'}]),
    ('cliproxy_partial_yaml', None, None, [{'jev_aux_cliproxy': True}]),
]
proxy_files = {
    'cliproxy_access': 'api-keys:\n  - root-key\naccess:\n  api-keys:\n    - access-key\n',
    'cliproxy_overrides_body': 'api-keys:\n  - file-key\n',
    'cliproxy_partial_yaml': 'port: [broken\napi-keys:\n  - partial-key\n',
}
cases = []
defaults = None
for name, initial, settings, steps in scenarios:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, mock_patch.dict(os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True):
        home = Path(folder)
        config = home / '.claude'
        config.mkdir()
        mp.setattr(rc, '_backend_config_base', lambda: config)
        mp.setattr(rc, '_cache', None)
        mp.setattr(pensamento, 'compartilhado', lambda: config)
        mp.setattr(cc, '_config_files', lambda: [home / '.claude.json'])
        mp.setattr(cc, '_main_file', lambda: home / '.claude.json')
        mp.setattr(cc, '_parked_file', lambda: home / '.hangar/computer-control.json')
        mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
        if defaults is None:
            defaults = {key: getattr(rc.settings, key, None) for key in rc.EDITAVEIS}
        runtime = config / 'runtime-config.json'
        thinking = config / 'settings.json'
        proxy = proxy_files.get(name)
        if proxy is not None:
            proxy_path = home / '.cli-proxy-api/config.yaml'
            proxy_path.parent.mkdir()
            proxy_path.write_text(proxy)
        if initial is not None:
            runtime.write_text(initial)
        if settings is not None:
            thinking.write_text(settings)
        observations = []
        for body in steps:
            before = rc.estado()
            try:
                value = rc.aplicar({k:v for k,v in body.items() if v is not None}, remover={k for k,v in body.items() if v is None})
                result = {'ok': value}
            except Exception as exc:
                result = {'error': str(exc), 'kind': type(exc).__name__}
            observations.append({'body': body, 'before': before, 'result': result, 'after': rc.estado(),
                'runtime': runtime.read_text() if runtime.exists() else None,
                'settings': thinking.read_text() if thinking.exists() else None})
        case = {'name': name, 'initial': initial, 'settings': settings, 'steps': observations}
        if proxy is not None:
            case['proxy'] = proxy
        cases.append(case)
result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
          'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_runtime.py',
          'injections': ['ambiente vazio', 'casas temporárias', 'scan_roots vazio'],
          'defaults': defaults, 'cases': cases}
Path(__file__).with_name('runtime.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')

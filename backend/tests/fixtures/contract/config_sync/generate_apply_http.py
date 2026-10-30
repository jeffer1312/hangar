"""Captura a borda de aplicação com pacote real e efeitos em uma pasta isolada."""
import asyncio
import base64
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from fastapi import FastAPI
from fastapi.testclient import TestClient
from pytest import MonkeyPatch
from app import config_sync as cs, config_sync_api as api, runtime_config as rc
from app.config import Settings

payload = cs.pack(cs.Bundle(items={'hangar_prefs': {'prefs': {'editor': 'vim'}}}))
scenarios = [(method, '?items=hangar_prefs', 'valid', False)
             for method in ['GET', 'HEAD', 'OPTIONS', 'PUT', 'PATCH', 'DELETE', 'POST']]
for query in ['', '?items=', '?items=unknown', '?items=hangar_prefs,unknown',
              '?items=,hangar_prefs,,', '?items=hangar_prefs,hangar_prefs',
              '?items=hangar_prefs&stream=true', '?items=hangar_prefs&stream=no',
              '?items=hangar_prefs&stream=invalid', '?items=hangar_prefs&stream=1&stream=false']:
    for body in ['valid', 'empty', 'invalid']:
        scenarios.append(('POST', query, body, False))
scenarios += [('POST', '?items=hangar_prefs', body, True) for body in ['valid', 'empty', 'invalid']]
cases = []
for method, query, body_kind, busy in scenarios:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(
        os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True
    ):
        home = Path(folder)
        claude = home / '.claude'
        roots = cs.Roots(home=str(home), claude=str(claude), codex=str(home / '.codex'), hangar=str(home / 'repo'))
        mp.setattr(api, 'Roots', SimpleNamespace(this_machine=lambda: roots))
        mp.setattr(api, '_APPLYING', asyncio.Lock())
        mp.setattr(rc, '_backend_config_base', lambda: claude)
        mp.setattr(rc, '_cache', None)
        mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
        mp.setattr(cs, '_new_backups', lambda _: home / '.hangar/config-sync/backups/fixture')
        calls = []
        unpack = cs.unpack
        apply_bundle = cs.apply_bundle

        def observed_unpack(raw):
            calls.append('unpack')
            return unpack(raw)

        async def observed_apply(*args, **kwargs):
            calls.append('apply')
            return await apply_bundle(*args, **kwargs)

        async def accepted_auth():
            calls.append('auth')

        mp.setattr(cs, 'unpack', observed_unpack)
        mp.setattr(cs, 'apply_bundle', observed_apply)
        app = FastAPI()
        app.include_router(api.config_sync_router)
        app.dependency_overrides[api.require_auth] = accepted_auth
        raw = {'valid': payload, 'empty': b'', 'invalid': b'invalid'}[body_kind]
        with TestClient(app, raise_server_exceptions=False) as client:
            if busy:
                client.portal.call(api._APPLYING.acquire)
            response = client.request(method, '/api/config-sync/apply' + query, content=raw)
            if busy:
                client.portal.call(api._APPLYING.release)
        try:
            body = response.json()
        except ValueError:
            body = response.text
        body = json.loads(json.dumps(body).replace(folder, '⟦HOME⟧'))
        files = {str(path.relative_to(home)).replace('\\', '/'): path.read_text()
                 for path in home.rglob('*') if path.is_file()}
        cases.append({'method': method, 'query': query, 'input': body_kind, 'busy': busy,
                      'status': response.status_code, 'body': body, 'calls': calls, 'files': files,
                      'headers': {key: response.headers[key] for key in ['allow', 'content-type', 'content-encoding', 'cache-control'] if key in response.headers}})

Path(__file__).with_name('apply-http.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_apply_http.py',
    'injections': ['autenticação aceita e contada; sua autorização tem contrato próprio', 'backup fixture', 'raízes temporárias'],
    'payload_base64': base64.b64encode(payload).decode(), 'cases': cases,
}, ensure_ascii=True, indent=2) + '\n')

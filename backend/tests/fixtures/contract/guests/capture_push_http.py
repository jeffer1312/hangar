"""Caracteriza portas push originais, sem backend vivo nem envio de notificações."""
import ast
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from fastapi import Depends, FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field

sys.path.insert(0, str(Path.cwd()))
source = Path('app/api.py')
with tempfile.TemporaryDirectory() as scratch:
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch,
                      CP_AUTH_TOKEN='owner', CP_LAN_BIND_IP='127.0.0.1')
    os.environ.pop('HANGAR_AUTH_CONTEXT', None)
    from app import push
    from app.auth import require_auth, reset_backoff
    push.settings.projects_dir = root / 'projects'
    push.settings.vapid_public = 'fixture-public-key'
    app = FastAPI()
    app.add_middleware(CORSMiddleware, allow_origins=['*'], allow_methods=['*'], allow_headers=['*'],
                       expose_headers=['ETag', 'x-hangar-css-width', 'x-hangar-css-height'])
    names = {'_StrictBody', 'PushSubscribeBody', 'PushMuteBody', 'PushQuietHoursBody',
             'push_vapid', 'push_subscribe', 'push_settings', 'push_mute', 'push_quiet_hours'}
    nodes = [n for n in ast.parse(source.read_text()).body if isinstance(n, (ast.ClassDef, ast.FunctionDef)) and n.name in names]
    ns = dict(app=app, Depends=Depends, require_auth=require_auth, BaseModel=BaseModel,
              ConfigDict=ConfigDict, Field=Field, HTTPException=HTTPException, push=push, settings=push.settings)
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), 'exec'), ns)
    valid = {'vapid': None, 'settings': None,
             'subscribe': {'subscription': {'endpoint': 'https://fixture.invalid/e', 'keys': {'auth': 'fixture'}}, 'label': 'Servidor', 'serverId': 'fixture'},
             'mute': {'session': 's', 'muted': True}, 'quiet-hours': {'start': '22:00', 'end': '07:00'}}
    cases = []
    for route, body in valid.items():
        for method in ['GET', 'HEAD', 'POST', 'PUT', 'DELETE', 'OPTIONS', 'PATCH']:
            for suffix in ['', '/']:
                cases.append(dict(route=route+suffix, method=method, token='owner', media='application/json', raw=json.dumps(body).encode() if body else b''))
        if body is not None:
            bodies = [b'', b'{', b'\xff', b'null', b'[]', b'{}', json.dumps(body).encode()]
            for field in body:
                for value in [None, False, 0, 1.0, [], {}, '', '\ud800', 'texto']:
                    altered = body | {field: value}
                    bodies.append(json.dumps(altered).encode())
            for media in [None, 'text/plain', 'application/json', 'application/vnd.fixture+json']:
                for token in ['owner', 'invalid']:
                    for raw in bodies:
                        cases.append(dict(route=route, method='POST', token=token, media=media, raw=raw))
    for endpoint in [None, False, 0, 1, [], ["e"], {}, {"e": 1}, "", "é", "\ud800"]:
        body = valid['subscribe'] | {'subscription': {'endpoint': endpoint}}
        cases.append(dict(route='subscribe', method='POST', token='owner', media='application/json', raw=json.dumps(body).encode()))
    for clock in ['00', '0000', '000000', 'T00:00', 't00:00', '24:00', '23:59:59.1234567', '23:59:59,5', '12.5', '12:30.5', '12:30:59.', '12:30Z', '12:30+01:00', '12:30-00:00:30', '12:30+24:00', ' 12:30', '12:30 ', '12:60', '1:00', '12:3', '12:30\u0000']:
        cases.append(dict(route='quiet-hours', method='POST', token='owner', media='application/json', raw=json.dumps({'start': clock, 'end': '07:00'}).encode()))
    for route in ['subscribe', 'mute', 'settings', 'quiet-hours']:
        file = 'subs.json' if route == 'subscribe' else 'push_prefs.json'
        for persisted in ['{', 'null', '[]', '1', '"texto"', '{}', '{"muted":null}', '{"muted":[1,"s"]}', '{"extra":NaN}', '[{}]', '[null]']:
            cases.append(dict(route=route, method='GET' if route=='settings' else 'POST', token='owner', media='application/json', raw=json.dumps(valid[route]).encode(), seed={file:persisted}))
    rows = []
    with TestClient(app, raise_server_exceptions=False) as client:
        for case in cases:
            reset_backoff()
            for file in (root / '.hangar-push').glob('*'):
                file.unlink()
            for name, text in case.get('seed', {}).items():
                (root / '.hangar-push').mkdir(exist_ok=True)
                (root / '.hangar-push' / name).write_text(text, encoding='utf-8')
            headers = {'authorization': 'Bearer '+case['token'], 'origin': 'https://fixture.invalid'}
            if case['media']: headers['content-type'] = case['media']
            response = client.request(case['method'], '/api/push/'+case['route'], content=case['raw'], headers=headers, follow_redirects=False)
            rows.append({k:v for k,v in case.items() if k != 'raw'} | {'body_hex': case['raw'].hex(),
                        'status': response.status_code, 'body': response.text, 'headers': dict(response.headers),
                        'files': {file.name: file.read_bytes().hex() for file in sorted((root / '.hangar-push').glob('*'))}})
    output = dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'], text=True).strip(),
                  command='cd backend && uv run python tests/fixtures/contract/guests/capture_push_http.py',
                  isolation='Modelos e handlers originais por AST, require_auth e persistência push reais; HOME temporário, chave pública fictícia; sem lifespan, envio, convidado ou middleware de limite. Backoff reiniciado por caso.', rows=rows)
    Path(__file__).with_name('push-http.json').write_text(json.dumps(output, ensure_ascii=True, indent=2)+'\n')
    print(len(rows))

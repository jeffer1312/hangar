"""Captura métodos, roteamento e precedência do porteiro antes das dependências."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch)
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',CP_SYNC='1',
                      CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config,guest_users,guest_users_api,guest_user_gate
    runtime_config._backend_config_base=lambda:root
    guest_users._path_override=root/'guests.json'
    with patch('secrets.token_hex',lambda n:'ab'*n),patch('secrets.token_urlsafe',lambda n:'guest-token'):
        guest_users.create('Ana',scratch,False,True)
    app=FastAPI();app.include_router(sync.sync_router);app.include_router(sync.sync_admin_router);app.include_router(guest_users_api.router);app.add_middleware(guest_user_gate.GuestUserGate)
    scope_box={}
    async def captured_app(scope,receive,send):
        if scope['type']=='http':scope_box['scope']=scope.copy()
        await app(scope,receive,send)
    client=TestClient(captured_app,client=("127.0.0.1",12345),raise_server_exceptions=False)
    paths=['/api/me','/api/guests','/api/guests/abc','/api/sync/status','/api/sync/setup','/api/sync/setup/disable',
           '/api/sync/vault','/api/sync/guests','/api/sync/guests/ana/delete','/api/sync/nope']
    paths += ['/api/me/','/api/guests/','/api/guests/abc/','/api/guests//','/api/sync/setup/',
              '/api/sync/nope/','/api/guests/a%2Fb','/api/guests/%C3%A9','/api/guests/%FF',
              '/api/sync/guests/ana/delete/','/api/me/?x=1%202','/api/guests/abc//?x=%2F']
    rows=[]
    for path in paths:
        for method in ['GET','HEAD','POST','PUT','PATCH','DELETE','OPTIONS']:
            for token in ['owner','bad','guest-token']:
                (root/'runtime-config.json').unlink(missing_ok=True)
                response=client.request(method,path,content='{}',headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'},follow_redirects=False)
                try:body=response.json()
                except ValueError:body=response.text
                from starlette.routing import Match
                full=any(route.matches(scope_box['scope'])[0]==Match.FULL for route in app.routes)
                partial=any(route.matches(scope_box['scope'])[0]==Match.PARTIAL for route in app.routes)
                location=response.headers.get('location')
                rows.append({'path':path,'decoded_path':scope_box['scope']['path'],'method':method,'token':token,
                             'matched':full,'partial':partial,'status':response.status_code,'response':body,
                             'allow':response.headers.get('allow'),'content_type':response.headers.get('content-type'),
                             'content_length':response.headers.get('content-length'),
                             'location':location.replace('http://testserver','⟦BASE⟧') if location else None})
    def route_table(routes):
        for route in routes:
            if hasattr(route,'original_router'):
                yield from route_table(route.original_router.routes)
            elif getattr(route,'path','').startswith('/api/'):
                for method in sorted(route.methods):yield [route.path,method]
    result={'source_commit' :subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_methods.py',
            'routes':list(route_table(app.routes)),'rows':rows}
    (HERE/'methods.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} respostas de roteamento capturadas')

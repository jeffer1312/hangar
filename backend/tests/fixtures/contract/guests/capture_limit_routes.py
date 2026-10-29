"""Compara o limite externo com autenticação e parser FastAPI reais, sem lifespan."""
import ast
import asyncio
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
source=HERE.parents[3]/'app/api.py'
nodes=[n for n in ast.parse(source.read_text()).body if isinstance(n,ast.ClassDef) and n.name in {'_BodyTooLarge','_BodySizeLimitMiddleware'}]
ns={};exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),'exec'),ns)
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner')
    from fastapi import FastAPI
    from app import guest_users,guest_users_api,guest_user_gate
    guest_users._path_override=Path(scratch)/'guests.json'
    guest,token=guest_users.create('Ana',scratch,False,True)
    app=FastAPI();app.include_router(guest_users_api.router);app.add_middleware(guest_user_gate.GuestUserGate)
    app.add_middleware(ns['_BodySizeLimitMiddleware'],max_bytes=8)
    async def capture():
        rows=[]
        for who,key in [('owner','owner'),('unknown','unknown'),('guest',token)]:
            for method,path,kind in [(method,path,kind) for method,path in [('POST','/api/guests'),('GET','/api/me')] for kind in ['content_length','chunked']]:
                sent=[];received=False
                async def receive():
                    nonlocal received
                    if received:return {'type':'http.disconnect'}
                    received=True;return {'type':'http.request','body':b'{"x":123}','more_body':False}
                async def send(message):sent.append(message)
                headers=[(b'authorization',('Bearer '+key).encode()),(b'content-type',b'application/json')]
                if kind=='content_length':headers.append((b'content-length',b'9'))
                scope={'type':'http','asgi':{'version':'3.0'},'http_version':'1.1','method':method,'scheme':'http',
                       'path':path,'raw_path':path.encode(),'query_string':b'','root_path':'','headers':headers,
                       'client':('127.0.0.1',1234),'server':('fixture',80)}
                await app(scope,receive,send)
                start=next(m for m in sent if m['type']=='http.response.start')
                raw=b''.join(m.get('body',b'') for m in sent if m['type']=='http.response.body').decode()
                try:body=json.loads(raw)
                except ValueError:body=raw
                rows.append({'method':method,'path':path,'who':who,'kind':kind,'limit':8,'input_bytes':9,'status':start['status'],'response':body,'consumed':received})
        return rows
    rows=asyncio.run(capture())
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_limit_routes.py','rows':rows}
    (HERE/'limit-routes.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print([(r['method'],r['who'],r['kind'],r['status']) for r in rows])

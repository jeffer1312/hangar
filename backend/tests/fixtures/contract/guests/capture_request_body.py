"""Captura Content-Type e codificação de JSON na borda FastAPI real."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch)
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',CP_SYNC='1',CP_SYNC_DATA=str(root/'vault.json'))
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_admin_router);client=TestClient(app,raise_server_exceptions=False)
    cases=[(kind,b'{}') for kind in [None,'application/json','application/json; charset=iso-8859-1','application/vnd.api+json','text/plain','application/octet-stream','TEXT/PLAIN','text/json']]
    cases.extend([('application/json','{}'.encode(codec)) for codec in ['utf-16','utf-16-le','utf-32','utf-32-be']])
    cases.extend([('application/octet-stream',b'\xff'),('application/json',b'\xff')])
    for codec in ['utf-8', 'utf-16', 'utf-16-le', 'utf-32']:
        for source in ['{"user":"\ud800"}', '{"unknown":"\ud800"}', '\ud800', '{"user":\ud800}', '{"user":"\ud800",}', '{"user":"\ud800"} trailing']:
            cases.append(('application/json',source.encode(codec,errors='surrogatepass')))
    for codec in ['utf-8', 'utf-16', 'utf-16-le', 'utf-32']:
        for source in ['{"\ud800":"value"}', '{"nested":["\ud800"]}', '{"normal":"normal"}']:
            cases.append(('application/json',source.encode(codec,errors='surrogatepass')))
    def contains_surrogate(value):
        if isinstance(value,str):return any(0xd800<=ord(char)<=0xdfff for char in value)
        if isinstance(value,list):return any(contains_surrogate(item) for item in value)
        if isinstance(value,dict):return any(contains_surrogate(key) or contains_surrogate(item) for key,item in value.items())
        return False
    rows=[]
    for content_type,raw in cases:
        headers={'Authorization':'Bearer owner'}
        if content_type is not None:headers['Content-Type']=content_type
        response=client.post('/api/sync/setup',content=raw,headers=headers)
        try:body=response.json()
        except ValueError:body=response.text
        try:
            decoded=json.loads(raw)
            parser={'accepted':True,'value_ascii':ascii(decoded),'contains_surrogate':contains_surrogate(decoded)}
        except (ValueError,UnicodeError) as exc:
            parser={'accepted':False,'exception':type(exc).__name__,'message':str(exc)}
        failure=None
        if response.status_code==500:
            try:
                TestClient(app,raise_server_exceptions=True).post('/api/sync/setup',content=raw,headers=headers)
            except Exception as exc:
                import traceback
                failure={'exception':type(exc).__name__,'message':str(exc),
                         'frames':[{'file':Path(frame.filename).name,'function':frame.name} for frame in traceback.extract_tb(exc.__traceback__)[-4:]]}
        rows.append({'content_type':content_type,'raw_hex':raw.hex(),'status':response.status_code,'response':body,'parser':parser,'failure':failure})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_request_body.py','rows':rows}
    (HERE/'request-body.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} corpos e tipos de conteúdo capturados')

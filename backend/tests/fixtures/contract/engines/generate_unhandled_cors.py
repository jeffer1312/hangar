"""Captura CORS fora de falhas tratadas e falhas do ServerErrorMiddleware original."""
import json
from pathlib import Path
import subprocess
from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from fastapi.testclient import TestClient

ROOT=Path(__file__).resolve().parents[5]
app=FastAPI()
app.add_middleware(CORSMiddleware,allow_origins=["*"],allow_methods=["*"],allow_headers=["*"],
                   expose_headers=["ETag","x-hangar-css-width","x-hangar-css-height"])
@app.get('/{kind}')
def endpoint(kind:str):
    if kind=='surrogate':raise HTTPException(422,{"input":"\ud800"})
    if kind=='nonfinite':raise HTTPException(422,{"input":float('nan')})
    if kind=='exception':raise ValueError('fixture')
    if kind=='handled-http':raise HTTPException(500,{"code":"fixture"})
    return JSONResponse({"code":"fixture"},status_code=500)
client=TestClient(app,raise_server_exceptions=False)
cases=[]
for kind in ('surrogate','nonfinite','exception','handled-http','handled-response'):
    base=client.get('/'+kind)
    response=client.get('/'+kind,headers={'origin':'https://fixture.invalid'})
    cases.append(dict(name=kind,unhandled=kind in ('surrogate','nonfinite','exception'),
        base=dict(status=base.status_code,headers=dict(base.headers),body=base.text),
        expected=dict(status=response.status_code,headers=dict(response.headers),body=response.text)))
Path(__file__).with_name('unhandled-cors.json').write_text(json.dumps(dict(
 source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
 command='uv run --with httpx python tests/fixtures/contract/engines/generate_unhandled_cors.py',cases=cases),ensure_ascii=True,indent=2)+'\n')

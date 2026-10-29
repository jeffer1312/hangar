"""Captura a classificação MIME do FastAPI original pela borda ASGI."""
import argparse,asyncio,json,subprocess
from pathlib import Path
from fastapi import FastAPI
import fastapi,httpx
p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);a=p.parse_args();app=FastAPI();received=[]
@app.middleware("http")
async def record_headers(request,call_next):
 received.clear();received.extend(value.hex() for key,value in request.scope["headers"] if key.lower()==b"content-type")
 return await call_next(request)
@app.put('/body')
def accept(body:dict):return {'accepted':True}
variants=[[],[b''],[b'application/json'],[b'application/json; charset=utf-8'],[b'application/vnd.fixture+json'],[b'Application/JSON'],[b'text/plain'],[b'text/json'],[b'application/octet-stream'],[b' application/json '],[b'application /json'],[b'application/ json'],[b'application/+json'],[b'application/json/foo'],[b'application/x+json; broken'],[b'"application/json"'],[b'application/json (comment)'],[b'application/foo(comment)+json'],[b'application/json; charset=\xff'],[b'\xa0application/json\xa0'],[b'\x85application/json\x85'],[b'application/json',b'text/plain'],[b'text/plain',b'application/json']]
async def capture():
 rows=[]
 async with httpx.AsyncClient(transport=httpx.ASGITransport(app,raise_app_exceptions=False),base_url='http://fixture') as client:
  for values in variants:
   response=await client.put('/body',content=b'{}',headers=[(b'content-type',value) for value in values]);rows.append(dict(headers_hex=[v.hex() for v in values],received_headers_hex=list(received),status=response.status_code,body=response.text))
 return rows
a.output.write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),fastapi=fastapi.__version__,command='uv run --project backend --with httpx python backend/tests/fixtures/contract/http-body-input/generate-mime.py --output backend/tests/fixtures/contract/http-body-input/mime.json',cases=asyncio.run(capture())),ensure_ascii=True,indent=2)+'\n');print(f'{len(variants)} casos MIME capturados')

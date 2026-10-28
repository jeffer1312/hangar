"""Caracteriza o servidor estático Python do commit de origem, sem subir o backend."""
import ast
import json
import os
import subprocess
import tempfile
from pathlib import Path
from fastapi import FastAPI
from fastapi.staticfiles import StaticFiles
from fastapi.testclient import TestClient
from starlette.responses import Response

ROOT = Path(__file__).resolve().parents[5]
COMMIT = 'ba2db1021'
source = subprocess.check_output(['git', 'show', f'{COMMIT}:backend/app/api.py'], cwd=ROOT, text=True)
node = next(n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == '_UIStatic')
namespace = dict(StaticFiles=StaticFiles, Path=Path, Response=Response)
exec(compile(ast.Module(body=[node], type_ignores=[]), '<referência>', 'exec'), namespace)
files = {'index.html': '<p>tela</p>', 'assets/app.js': '0123456789', 'sub/index.html': '<p>sub</p>', 'sw.js': 'worker', 'note.txt': 'texto', 'favicon.ico': 'icone', 'manifest.webmanifest': '{}', 'LICENSE': 'licença'}
cases = [('root','GET','/',{}),('head','HEAD','/',{}),('asset','GET','/assets/app.js',{}),('worker','GET','/sw.js',{}),('favicon','GET','/favicon.ico',{}),('manifest','GET','/manifest.webmanifest',{}),('unknown-mime','GET','/LICENSE',{}),('missing','GET','/missing',{}),('long-name','GET','/'+('x'*300),{}),('method','POST','/',{}),('directory','GET','/sub?x=1',{'x-hangar-static-origin':'https://forged.example'}),('sub','GET','/sub/',{}),('range','GET','/assets/app.js',{'range':'bytes=2-5'}),('suffix','GET','/assets/app.js',{'range':'bytes=-3'}),('open','GET','/assets/app.js',{'range':'bytes=7-'}),('invalid-range','GET','/assets/app.js',{'range':'bytes=99-'}),('wrong-unit','GET','/assets/app.js',{'range':'items=0-1'}),('empty-range','GET','/assets/app.js',{'range':'bytes=x'}),('if-range-miss','GET','/assets/app.js',{'range':'bytes=0-1','if-range':'"other"'}),('range-empty','GET','/assets/app.js',{'range':'bytes=5-4'}),('range-negative','GET','/assets/app.js',{'range':'bytes=-0'}),('range-many','GET','/assets/app.js',{'range':'bytes=0-1,8-9'}),('ims','GET','/',{'if-modified-since':'Tue, 14 Nov 2023 22:13:20 GMT'}),('inm-wins','GET','/',{'if-modified-since':'Tue, 14 Nov 2023 22:13:20 GMT','if-none-match':'"other"'})]
with tempfile.TemporaryDirectory() as tmp:
 root=Path(tmp)
 for name,text in files.items():
  p=root/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(text);os.utime(p,(1700000000,1700000000))
 app=FastAPI();app.mount('/',namespace['_UIStatic'](directory=root,html=True));client=TestClient(app,follow_redirects=False)
 captured=[]
 for name,method,path,headers in cases:
  r=client.request(method,path,headers=headers)
  body=r.text
  if r.headers.get('content-type','').startswith('multipart/byteranges'):
   boundary=r.headers['content-type'].split('boundary=')[1]
   body=body.replace(boundary,'BOUNDARY')
  selected={k:v for k,v in r.headers.items() if k in ['content-type','content-length','cache-control','last-modified','accept-ranges','content-range','location']}
  if 'boundary=' in selected.get('content-type',''):
   selected['content-type']='multipart/byteranges; boundary=BOUNDARY'
   selected.pop('content-length',None)
  captured.append(dict(name=name,method=method,path=path,headers=headers,status=r.status_code,body=body,response_headers=selected))
 out=dict(commit=COMMIT,command='uv run python tests/fixtures/contract/web_static/capture.py',files=files,mtime=1700000000,cases=captured)
 Path(__file__).with_name('reference.json').write_text(json.dumps(out,ensure_ascii=False,indent=2)+'\n')

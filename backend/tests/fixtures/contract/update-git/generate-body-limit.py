"""Captura o middleware real por ASGI, sem transmitir corpos grandes ou iniciar serviços."""
import ast,asyncio,json,subprocess,sys
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
source=ast.parse((reference/'backend/app/api.py').read_text());namespace={}
exec(compile(ast.Module(body=[n for n in source.body if isinstance(n,ast.ClassDef) and n.name in ['_BodySizeLimitMiddleware','_BodyTooLarge']],type_ignores=[]),'original-body-limit','exec'),namespace)
LIMIT=100*1024*1024
async def capture(header,chunks,authorized):
 sent=[];reads=0;entered=0
 async def app(scope,receive,send):
  nonlocal entered;entered+=1
  if authorized:
   while (await receive()).get('more_body'):pass
  await send({'type':'http.response.start','status':200 if authorized else 401,'headers':[]})
  await send({'type':'http.response.body','body':b'OK' if authorized else b'unauthorized'})
 async def receive():
  nonlocal reads
  count=chunks[reads] if reads<len(chunks) else 0;reads+=1
  return {'type':'http.request','body':b'x'*count,'more_body':reads<len(chunks)}
 async def send(message):sent.append(message)
 headers=[] if header is None else [(b'content-length',header.encode())]
 await namespace['_BodySizeLimitMiddleware'](app,max_bytes=LIMIT)({'type':'http','headers':headers},receive,send)
 return dict(header=header,chunks=chunks,authorized=authorized,entered=entered,reads=reads,status=sent[0]['status'],headers={k.decode():v.decode() for k,v in sent[0]['headers']},body=b''.join(m.get('body',b'') for m in sent).decode())
async def main():
 cases=[]
 for header in [None,'',str(LIMIT),str(LIMIT+1),'9'*100,'0'*100,'0'*100+str(LIMIT),'-1','+123',' 123','abc']:
  for auth in [False,True]:cases.append(await capture(header,[0],auth))
 for chunks in [[LIMIT],[LIMIT,1],[LIMIT//2,LIMIT//2,1]]:
  for auth in [False,True]:cases.append(await capture(None,chunks,auth))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-body-limit.py <checkout-Python> <fixture>',limit=LIMIT,cases=cases),ensure_ascii=False,indent=2)+'\n');print(f'{len(cases)} contratos de limite capturados')
asyncio.run(main())

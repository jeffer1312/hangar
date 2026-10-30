#!/usr/bin/env python3
"""Confere efeitos do toggle do painel sobre um livro temporário."""
import hashlib,json,os,subprocess,sys,tempfile
from pathlib import Path
binary=Path(sys.argv[1]).resolve();fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel-write.json').read_text());failures=[]
for row in fixture['cases']:
 with tempfile.TemporaryDirectory() as tmp:
  config=Path(tmp)/'.hangar/config';config.mkdir(parents=True);path=config/'peers.json';path.write_text(row['input']);path.chmod(0o640);(config/'peers.json.stale.tmp').write_text('synthetic')
  result=subprocess.run([str(binary),'panel-action','peer','peer','on'],env={**os.environ,'HOME':tmp,'USERPROFILE':tmp},cwd=tmp,capture_output=True,timeout=15)
  expected=row['result'];actual=path.read_bytes();issues=[]
  if result.returncode!=expected['rc']:issues.append('rc')
  if result.stdout.hex()!=expected['stdout_hex']:issues.append('stdout')
  if 'file_hex' in expected and actual.hex()!=expected['file_hex']:issues.append('arquivo')
  if 'file_sha256' in expected and (len(actual)!=expected['file_bytes'] or hashlib.sha256(actual).hexdigest()!=expected['file_sha256']):issues.append('arquivo-profundo')
  if path.stat().st_mode&0o777!=expected['mode']:issues.append('modo')
  temps=[p.name for p in config.glob('*.tmp')]
  if temps:issues.append('temporários')
  if row['name']=='surrogate' and result.stderr!=b'erro: livro de peers cont\xc3\xa9m Unicode inv\xc3\xa1lido\n':issues.append('diagnóstico-D45')
  if issues:failures.append((row['name'],issues,result.stdout[:300].decode(errors='replace')))
assert not failures,failures
print('10 escritas do painel conferidas: bytes, modo, saída, retorno e temporários; D45 específica')

from concurrent.futures import ThreadPoolExecutor
with tempfile.TemporaryDirectory() as tmp:
 config=Path(tmp)/'.hangar/config';config.mkdir(parents=True);path=config/'peers.json'
 initial={'pc':{'base_url':'http://pc','token':'fixture-pc','web_url':'https://pc'},'srv':{'base_url':'http://srv','token':'fixture-srv'}}
 path.write_text(json.dumps(initial));path.chmod(0o600)
 environment={**os.environ,'HOME':tmp,'USERPROFILE':tmp}
 def toggle(name,state):
  completed=subprocess.run([str(binary),'panel-action',name,'peer',state],env=environment,cwd=tmp,capture_output=True,timeout=10)
  assert completed.returncode==0,(name,completed.stdout,completed.stderr)
 with ThreadPoolExecutor(max_workers=2) as pool:list(pool.map(lambda name:toggle(name,'off'),['pc','srv']))
 actual=json.loads(path.read_text())
 for name in initial:assert actual[name]=={**initial[name],'enabled':False}
 toggle('pc','on');actual=json.loads(path.read_text())
 assert actual['pc']=={**initial['pc'],'enabled':True}
 assert actual['srv']=={**initial['srv'],'enabled':False}
 assert path.stat().st_mode&0o777==0o600
 assert not list(config.glob('*.tmp'))
print('Concorrência entre processos, vizinhos, tokens, modo0600 e ida/volta conferidos')

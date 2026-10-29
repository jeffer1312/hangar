"""Exercita o bootstrap POSIX sem rede, comparando com a captura do downloader Python."""
import hashlib,json,os,shutil,subprocess,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
CASES=json.loads((ROOT/'backend/tests/fixtures/contract/update-git/release-cases.json').read_text())['cases']
with tempfile.TemporaryDirectory(prefix='hangar-bootstrap-proof-') as scratch:
 base=Path(scratch)
 for case in CASES:
  name=case['name']
  if name=='hold':continue
  root=base/name;(root/'scripts').mkdir(parents=True);(root/'bin').mkdir();dest=root/'dest';dest.mkdir()
  shutil.copyfile(ROOT/'scripts/install-server-binaries.sh',root/'scripts/install-server-binaries.sh')
  if name in ('same','replace','branch-404-keeps'):
   for binary in ('hangar-server','hangar-cano'):(dest/binary).write_text(('new-' if name=='same' else 'old-')+binary)
  old=json.loads((ROOT/'backend/tests/fixtures/contract/update-git/bootstrap-old.json').read_text()) if name=='same' else None
  if old:
   for n,b in old['before'].items():(dest/n).write_text(b)
   (dest/'hangar-server.old-directory').mkdir()
  manifest={'files':{f'linux-x86_64/{binary}':{'name':binary,'sha256':hashlib.sha256(('new-'+binary).encode()).hexdigest()} for binary in ('hangar-server','hangar-cano')}}
  if name=='missing-entry':del manifest['files']['linux-x86_64/hangar-cano']
  (root/'manifest.json').write_text(json.dumps(manifest))
  curl='''#!/usr/bin/env node
const fs=require('fs'),path=require('path');const root=process.env.FIXTURE_ROOT,scenario=process.env.FIXTURE_CASE,args=process.argv.slice(2),url=args.at(-1),output=args[args.indexOf('--output')+1];let status=200,data;
if(url.endsWith('server-latest.json')) {
if(url.includes('server-beta/')&&scenario.startsWith('branch-404')){status=404;data='missing';}
else if(url.includes('server-beta/')&&scenario==='branch-no-platform')data='{"files":{}}';
else data=scenario==='manifest-list'?'[]':fs.readFileSync(path.join(root,'manifest.json'));
}else{if(scenario==='download-failure')process.exit(1);data=scenario==='hash-mismatch'?'wrong':'new-'+url.split('/').at(-1);}
fs.writeFileSync(output,data);if(args.includes('--write-out'))process.stdout.write(String(status));
'''
  programs={'curl':curl,'git':'#!/bin/sh\nprintf "%s\\n" "'+('beta' if name.startswith('branch-') else 'main')+'"\n','uname':'#!/bin/sh\ncase "$1" in -s) echo '+('Plan9' if name=='unsupported' else 'Linux')+';; -m) echo x86_64;; esac\n'}
  for binary,body in programs.items():file=root/'bin'/binary;file.write_text(body);file.chmod(0o755)
  env={**os.environ,'HOME':str(root),'USERPROFILE':str(root),'PATH':str(root/'bin')+os.pathsep+os.environ['PATH'],'HANGAR_SERVER_BIN_DIR':str(dest),'FIXTURE_ROOT':str(root),'FIXTURE_CASE':name}
  env.pop('HANGAR_SERVER_RELEASE_URL',None);env.pop('HANGAR_UPDATE_OWNS_RUST',None)
  result=subprocess.run(['bash',str(root/'scripts/install-server-binaries.sh')],env=env,capture_output=True,text=True)
  expected=2 if case['result'] is None else 1 if case['result'] else 0
  assert result.returncode==expected,(name,result.returncode,result.stdout,result.stderr)
  assert {p.name:p.read_text() for p in dest.iterdir() if p.is_file()}=={**case['after_publish'],**(old['after'] if old else {})},name
  if old:assert (dest/'hangar-server.old-directory').is_dir()
 print('11 cenários de bootstrap POSIX: códigos e bytes iguais ao Python capturado')

source=ROOT/'scripts/install-server-binaries.sh'
for case in json.loads((ROOT/'backend/tests/fixtures/contract/update-git/bootstrap-extra.json').read_text())['cases']:
 with tempfile.TemporaryDirectory() as tmp:
  root=Path(tmp);(root/'scripts').mkdir();(root/'bin').mkdir();shutil.copy(source,root/'scripts/install-server-binaries.sh')
  body='{' if case['name']=='branch-invalid-json' else '[]'
  programs={'git':'#!/bin/sh\necho beta\n','uname':'#!/bin/sh\n[ "$1" = -s ] && echo Linux || echo x86_64\n','curl':'''#!/usr/bin/env node
const fs=require('fs'),args=process.argv.slice(2);fs.appendFileSync(process.env.PROOF_LOG,args.at(-1)+'\\n');fs.writeFileSync(args[args.indexOf('--output')+1],process.env.PROOF_BODY);process.stdout.write('200');
'''}
  for n,b in programs.items():p=root/'bin'/n;p.write_text(b);p.chmod(0o755)
  env={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PATH':str(root/'bin')+os.pathsep+os.environ['PATH'],'PROOF_LOG':str(root/'calls'),'PROOF_BODY':body,'HANGAR_SERVER_BIN_DIR':str(root/'dest')}
  env.pop('HANGAR_SERVER_RELEASE_URL',None);env.pop('HANGAR_UPDATE_OWNS_RUST',None)
  p=subprocess.run(['bash',str(root/'scripts/install-server-binaries.sh')],env=env,capture_output=True)
  assert p.returncode==1
  calls=(root/'calls').read_text().splitlines()
  assert calls==case['urls'],(case['name'],calls,case['urls'])
print('2 recusas de manifesto: sem fallback nem gravação')

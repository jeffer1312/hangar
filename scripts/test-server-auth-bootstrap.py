"""Prova o primeiro upgrade com o par de autenticação sem rede nem backend."""
import json,os,shutil,subprocess,tempfile,hashlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
fixture=json.loads((ROOT/'backend/tests/fixtures/contract/update-git/auth-release.json').read_text())
failures=[]
for row in fixture['cases']:
 name=row['name']
 if name.startswith('hold-') or (name.endswith('publish-failure') and not name.startswith('branch-')):continue
 with tempfile.TemporaryDirectory() as temp:
  root=Path(temp);dest=root/'dest';dest.mkdir();(root/'scripts').mkdir();(root/'bin').mkdir()
  for file in ['install-server-binaries.sh','server-binaries-publish.cjs']:shutil.copy(ROOT/'scripts'/file,root/'scripts'/file)
  for file,data in row['initial'].items():target=dest/file;target.parent.mkdir(parents=True,exist_ok=True);target.write_text(data)
  old=json.loads((ROOT/'backend/tests/fixtures/contract/update-git/bootstrap-old.json').read_text()) if name=='same' else None
  if old:
   for file,data in old['before'].items():(dest/file).write_text(data)
   (dest/'hangar-server.old-directory').mkdir()
  manifest={'files':{f'linux-x86_64/{n}':{'name':n,'sha256':hashlib.sha256(('new-'+n).encode()).hexdigest()} for n in ['hangar-auth','hangar-cano','hangar-server']}}
  if name=='missing-auth':del manifest['files']['linux-x86_64/hangar-auth']
  if name=='invalid-auth-sha':manifest['files']['linux-x86_64/hangar-auth']['sha256']='invalid'
  (root/'manifest.json').write_text(json.dumps(manifest))
  (root/'failure.cjs').write_text('''const fs=require('fs');const original=fs.renameSync;let failed=false;fs.renameSync=function(src,target){const name=process.env.PROOF_CASE;if(!failed&&process.argv[1]?.endsWith('server-binaries-publish.cjs')&&((name==='branch-server-publish-failure'&&target.endsWith('/hangar-server'))||(name==='branch-metadata-publish-failure'&&target.endsWith('/hangar-server.auth.json')))){failed=true;throw new Error('fixture-publication-failed');}return original(src,target);};''')
  programs={'git':'#!/bin/sh\necho '+('beta' if name.startswith('branch-') else 'main')+'\n','uname':'#!/bin/sh\n[ "$1" = -s ] && echo Linux || echo x86_64\n','curl':'''#!/usr/bin/env node
const fs=require('fs'),path=require('path'),root=process.env.PROOF_ROOT,scenario=process.env.PROOF_CASE,args=process.argv.slice(2),url=args.at(-1),out=args[args.indexOf('--output')+1];fs.appendFileSync(path.join(root,'urls'),url+'\\n');let data,status=200;
if(url.endsWith('server-latest.json')){let doc=JSON.parse(fs.readFileSync(path.join(root,'manifest.json')));if(url.includes('/server-beta/')){if(['branch-keeps','branch-server-download-failure','branch-server-publish-failure','branch-metadata-publish-failure'].includes(scenario))status=404;if(scenario==='branch-missing-auth')delete doc.files['linux-x86_64/hangar-auth'];}data=JSON.stringify(doc);}
else{const n=url.split('/').at(-1),kind=n.replace('hangar-','');if(scenario.replace(/^branch-/, '')===kind+'-download-failure')process.exit(1);data=scenario===kind+'-hash-failure'?'wrong':'new-'+n;}fs.writeFileSync(out,data);if(args.includes('--write-out'))process.stdout.write(String(status));
'''}
  for n,b in programs.items():p=root/'bin'/n;p.write_text(b);p.chmod(0o755)
  env={**os.environ,'HOME':temp,'USERPROFILE':temp,'PATH':str(root/'bin')+os.pathsep+os.environ['PATH'],'HANGAR_SERVER_BIN_DIR':str(dest),'PROOF_ROOT':temp,'PROOF_CASE':name,'NODE_OPTIONS':'--require='+str(root/'failure.cjs')}
  for key in ['HANGAR_SERVER_RELEASE_URL','HANGAR_UPDATE_OWNS_RUST']:env.pop(key,None)
  p=subprocess.run(['bash',str(root/'scripts/install-server-binaries.sh')],env=env,capture_output=True,text=True)
  expected_messages=[msg for msg in row['result'] if 'instalei os da main' in msg]
  actual_messages=[msg for msg in p.stdout.splitlines() if 'instalei os da main' in msg]
  if actual_messages!=expected_messages:failures.append((name,'avisos de instalação',actual_messages,expected_messages))
  actual={str(p.relative_to(dest)):p.read_text() for p in dest.rglob('*') if p.is_file()}
  urls=(root/'urls').read_text().splitlines();expected_urls=[u.replace('https://release.invalid','https://github.com/jeffer1312/hangar/releases/download') for u in row['urls']]
  for key,got,want in [('code',p.returncode,int(bool(row['result']))),('files',actual,{**row['after_publish'],**(old['after'] if old else {})}),('urls',urls,expected_urls)]:
   if got!=want:failures.append((name,key,got,want))
assert not failures,failures
print('17 casos do bootstrap com autenticação: códigos, URLs e bytes conferidos')

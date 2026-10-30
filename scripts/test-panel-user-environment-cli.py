#!/usr/bin/env python3
"""Confere idioma herdado e expansão POSIX sem abrir seletor real."""
import json,os,pwd,subprocess,sys,tempfile
from pathlib import Path
root=Path(__file__).resolve().parent.parent
fixture=root/'backend/tests/fixtures/contract/update-git/panel-user-environment.json'
capture=sys.argv[1]=='--capture';target=sys.argv[2] if capture else str(Path(sys.argv[1]).resolve())
user=pwd.getpwuid(os.getuid());rows=[];failures=[]
cases=[(name,locale,scan,dialog) for name,locale,scan in [('lang-only',{'LANG':'pt_BR.UTF-8'},'~'),('locale-explicit',{'LANG':'pt_BR.UTF-8','LC_ALL':'C.UTF-8','LANGUAGE':'pt_BR:pt'},'~'),('locale-absent',{},'~'),('named-user',{},'~<USER>'),('named-user-parent',{},'~<USER>/..'),('unknown-user',{},'~hangar-fixture-user-does-not-exist/path'),('home-child',{},'~/chosen'),('home-double-slash',{},'~//chosen'),('named-double-slash',{},'~<USER>//.')] for dialog in ['kdialog','zenity']]
with tempfile.TemporaryDirectory() as reference:
 if capture:
  for name in ['hangar-panel-action','hangar_panel_common.py']:
   Path(reference,name).write_bytes(subprocess.check_output(['git','-C',str(root),'show',target+':scripts/'+name]))
 for name,locale,scan,dialog in cases:
  with tempfile.TemporaryDirectory() as tmp:
   home=Path(tmp);config=home/'.hangar/config';config.mkdir(parents=True);(home/'chosen').mkdir()
   (config/'.env').write_text('CP_SCAN_ROOTS='+scan.replace('<USER>',user.pw_name)+'\n')
   program=home/dialog;program.write_text(f'#!{sys.executable}\nimport json,os,sys\nfrom pathlib import Path\nPath(os.environ["TRACE"]).write_text(json.dumps({{"argv":sys.argv[1:],"locale":{{k:os.environ.get(k) for k in ["LANG","LC_ALL","LANGUAGE"]}}}}))\nprint("/chosen")\n');program.chmod(0o700)
   env={'HOME':tmp,'USERPROFILE':tmp,'PATH':tmp,'TRACE':str(home/'trace'),**locale}
   command=[sys.executable,str(Path(reference)/'hangar-panel-action')] if capture else [target,'panel-action']
   result=subprocess.run([*command,'fixture','pick-folder'],env=env,cwd=tmp,capture_output=True,timeout=10)
   def normalize(text):return text.replace(tmp,'<ROOT>').replace(user.pw_dir,'<USER_HOME>')
   observed={'rc':result.returncode,'stdout':normalize(result.stdout.decode()),'stderr':normalize(result.stderr.decode()),'trace':json.loads(normalize((home/'trace').read_text()))}
   rows.append({'name':name+'-'+dialog,'result':observed})
 if capture:fixture.write_text(json.dumps({'source_commit':target,'generator':'python3 scripts/test-panel-user-environment-cli.py --capture '+target,'cases':rows},ensure_ascii=False,indent=2)+'\n')
 else:
  expected=json.loads(fixture.read_text())['cases']
  for actual,wanted in zip(rows,expected,strict=True):
   if actual!=wanted:failures.append((actual,wanted))
  assert not failures,failures
print(f'{len(rows)} contratos de idioma e expansão conferidos')

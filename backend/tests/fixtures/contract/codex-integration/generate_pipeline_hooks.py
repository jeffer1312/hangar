"""Captura efeitos dos consumidores de hooks da reconciliação, sem instalar hooks reais."""
import io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
def document(*commands):return {'hooks':{'PreToolUse':[{'hooks':[{'type':'command','command':command} for command in commands]}]}}
cases=[]
scenarios=[
 dict(name='missing-empty',source={}),dict(name='missing-hooks',source={'hooks':{}}),dict(name='empty-existing',current={},source={}),
 dict(name='same',current=document('old'),source=document('old')),
 dict(name='merge',current=document('old','personal'),source=document('new'),previous=document('old')),
 dict(name='no-source',current=document('old'),source={}),
 dict(name='remove-managed',current=document('old','personal'),source={'hooks':{}},previous=document('old')),
 dict(name='legacy-missing',legacy=True),dict(name='legacy-no-target',legacy=True,mirror=document('old')),
 dict(name='legacy-remove',legacy=True,mirror=document('old'),current=document('old','personal')),
 dict(name='legacy-empty',legacy=True,mirror={},current=document('personal')),
 dict(name='legacy-install-failure',legacy=True,mirror=document('old'),current=document('old','personal'),install_error=True),
]
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir()
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT))) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(Path(raw)/'import-home');Path(os.environ['HOME']).mkdir();sys.path.insert(0,str(archive/'backend'))
 from app import codex_integracao as original,codex_hook_installer
 original.install_root.interpreter=lambda:'python-fixture'
 for scenario in scenarios:
  with tempfile.TemporaryDirectory() as folder:
   root=Path(folder);home=root/'home';account=home/'.codex';account.mkdir(parents=True);original._REPO=root/'repo'
   service=original.IntegracaoCodex(home=home,codex_home=account);service._estado=original._snapshot();events=[]
   for key,name in [('current','hooks.json'),('mirror','.hangar-hooks.json')]:
    if key in scenario:(account/name).write_text(json.dumps(scenario[key],indent=1))
   registry={'hooks':scenario.get('previous',{})}
   def install(path):
    events.append(['install',str(path).replace(str(root),'ROOT')])
    if scenario.get('install_error'):raise OSError('fixture')
   codex_hook_installer.ensure_codex_state_hook_installed=install
   try:
    if scenario.get('legacy'):service._migrar_ponte_antiga()
    else:service._hooks(scenario['source'],registry)
    result={'ok':True}
   except Exception as error:result={'error':type(error).__name__,'message':str(error)}
   files={str(p.relative_to(account)):p.read_bytes().hex() for p in sorted(account.rglob('*')) if p.is_file()}
   backups=sorted(p.read_text().replace(str(root),'ROOT').encode().hex() for p in service.backups.rglob('*') if p.is_file())
   result.update(events=events,files=files,backups=backups,registry=registry,pending=service._estado['confianca_pendente'],warnings=[original.serializar(v) for v in service._estado['avisos']])
   cases.append(dict(input=scenario,result=result))
Path(__file__).with_name('pipeline-hooks.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_pipeline_hooks.py',cases=cases),indent=2)+'\n')
print(len(cases))

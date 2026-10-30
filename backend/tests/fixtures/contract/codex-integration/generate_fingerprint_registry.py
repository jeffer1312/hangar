"""Captura validações de registros que o fingerprint executa antes de selecionar a saída."""
import io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
cases=[]
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir();data=subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT)
 with tarfile.open(fileobj=io.BytesIO(data)) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(Path(raw)/'import-home');Path(os.environ['HOME']).mkdir();sys.path.insert(0,str(archive/'backend'))
 from app import codex_integracao,skill_bridge
 for registry in [[],True,'text',{'plugins':None},{'plugins':[]},{'plugins':'text'},{'plugins':{'a':None,'b':{'path':3}}},{'artefatos':None},{'artefatos':True},{'artefatos':[3]},{'skills':None},{'skills':[3]}]:
  for phase in ['instructions','plugins','fragments','skills']:
   for sources in [False,True]:
    with tempfile.TemporaryDirectory() as directory:
     root=Path(directory);home=root/'home';account=home/'.codex';account.mkdir(parents=True);repo=root/'repo';repo.mkdir();codex_integracao._REPO=repo;skill_bridge._REPO=repo;codex_integracao.memoria_ligada=lambda:False;service=codex_integracao.IntegracaoCodex(home=home,codex_home=account)
     try:service._phase_fingerprints(fontes=sources,registry=registry,only=phase);result={'ok':True}
     except Exception as error:result={'error':type(error).__name__,'message':str(error)}
     cases.append(dict(registry=registry,phase=phase,sources=sources,result=result))
Path(__file__).with_name('fingerprint-registry.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_fingerprint_registry.py',cases=cases),indent=2)+'\n')
print(len(cases))

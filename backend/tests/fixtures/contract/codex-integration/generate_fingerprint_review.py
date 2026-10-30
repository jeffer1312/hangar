"""Captura leitura estrita de instruções e seleção de registros com semântica Windows."""
import io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path,PureWindowsPath
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
cases=[]
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir()
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT))) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(Path(raw)/'import-home');Path(os.environ['HOME']).mkdir();sys.path.insert(0,str(archive/'backend'))
 from app import codex_integracao,skill_bridge
 for path,data in [('.claude/CLAUDE.md',b'\xff'),('.claude/CLAUDE.MD',b'good\n\xff'),('.claude/import.md',b'\xc3'),('.claude/import.md',b'\xed\xa0\x80')]:
  for phase in ['instructions','plugins','fragments','skills']:
   with tempfile.TemporaryDirectory() as folder:
    root=Path(folder);home=root/'home';account=home/'.codex';repo=root/'repo';account.mkdir(parents=True);repo.mkdir();(home/'.claude').mkdir();(home/'.claude/CLAUDE.md').write_text('@./import.md\n');(home/path).write_bytes(data)
    codex_integracao._REPO=repo;skill_bridge._REPO=repo;codex_integracao.memoria_ligada=lambda:False
    service=codex_integracao.IntegracaoCodex(home=home,codex_home=account)
    try:service._phase_fingerprints(fontes=True,registry={},only=phase);result={'ok':True}
    except Exception as error:result={'error':type(error).__name__,'message':str(error)}
    cases.append(dict(path=path,hex=data.hex(),phase=phase,result=result))
 with tempfile.TemporaryDirectory() as folder:
  root=Path(folder);names=['one.json','two.JSON','three.JsOn','four.json.bak','filejson','.json','directory.JSON','long.jſon','unicode.ĵson']
  for name in names:
   if name.startswith('directory'):(root/name).mkdir()
   else:(root/name).write_text('{}')
  glob_cases=[]
  for windows in [False,True]:
   found=sorted(p.name for p in root.glob('*.json',case_sensitive=not windows))
   if windows:assert found==sorted(n for n in names if PureWindowsPath(n).full_match('*.json'))
   glob_cases.append(dict(windows=windows,names=names,found=found))
Path(__file__).with_name('fingerprint-review.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_fingerprint_review.py',platform='posix; glob Windows com case_sensitive=False e PureWindowsPath, sem VM',cases=cases,glob_cases=glob_cases),indent=2)+'\n')
print(len(cases),len(glob_cases))

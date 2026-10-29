"""Captura a versão de boot do Python sem usar git ou configuração da máquina."""
import json,os,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp);sys.path.insert(0,str(reference/'backend'))
 from app import diag
 diag._ROOT=Path(tmp);cases=[]
 for name,manifest,replies,version in [
  ('package',{'version':'0.1.2.3','commit':'a'*40,'channel':'latest'},[],None),
  ('short-commit',{'version':'custom','commit':'abc','channel':'beta'},[],None),
  ('checkout',None,[(0,'dist-latest-2-gabc-dirty\n'),(0,'42\n'),(0,'1.2.3\n')],None),
  ('nonzero-stdout',None,[(1,'visible\n'),(1,'42\n'),(1,'1.2.3\n')],None),
  ('fallback-version',None,[(0,'abc\n'),(0,'7\n'),(1,'')],'2.0.0\n'),
  ('unicode-count',None,[(0,'abc\n'),(0,'١٢\n'),(0,'1.2.3\n')],None),
  ('control-strip',None,[(0,'\x1cabc\x1f'),(0,'\x1e7\x1f'),(0,'\x1c1.2.3\x1f')],None),
  ('git-empty',None,[(1,''),(1,'')],None),
  ('git-exception',None,['error','error'],None),
  ('bad-manifest',[],[(0,'abc\n'),(0,'bad')],None),
  ('incomplete-manifest',{'version':'x'},[(0,'abc\n'),(0,'0\n'),(1,'')],None),
 ]:
  for item in ['manifest.json','VERSION']:(Path(tmp)/item).unlink(missing_ok=True)
  if manifest is not None:(Path(tmp)/'manifest.json').write_text(json.dumps(manifest))
  if version is not None:(Path(tmp)/'VERSION').write_text(version)
  pending=list(replies);calls=[]
  def run(args,**kwargs):
   calls.append(dict(args=args,timeout=kwargs['timeout']));r=pending.pop(0)
   if r=='error':raise OSError('synthetic unavailable')
   return subprocess.CompletedProcess(args,r[0],r[1],'')
  with patch.object(diag.subprocess,'run',run):describe=diag._git_describe();readable=diag.versao_legivel()
  cases.append(dict(name=name,manifest=manifest,replies=replies,version_file=version,calls=calls,describe=describe,readable=readable))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-boot-version.py <checkout-Python> <fixture>',cases=cases),ensure_ascii=False,indent=2)+'\n');print(f'{len(cases)} contratos de versão capturados')

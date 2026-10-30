"""Captura saída completa e registro limitado do comando, sem serviço ou backend real."""
import argparse,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument("--reference",type=Path,required=True);p.add_argument("--output",type=Path,required=True);a=p.parse_args();ref,out=a.reference.resolve(),a.output.resolve();commit=subprocess.check_output(["git","-C",str(ref),"rev-parse","HEAD"],text=True).strip()
with tempfile.TemporaryDirectory(prefix="hangar-command-log-") as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(root);sys.path.insert(0,str(ref/"backend"));from app import atualizar as app
 rows=[]
 for name,script in [
  ("backend","printf 'do backend\\n'"),
  ("motor-ansi","printf '\\033[31mvermelho\\033[0m\\n'; printf 'erro\\n' >&2"),
  ("motor-nonzero","printf 'falhou\\n' >&2; exit 7"),
  ("motor-many","i=1; while [ \"$i\" -le 805 ]; do printf '%s\\n' \"$i\"; i=$((i+1)); done")]:
  repo=root/name;repo.mkdir();state=repo/"state";state.mkdir();app.REPO=repo;app._base=lambda:state;app._SOU_O_MOTOR=name!="backend";app._escrever(fase="rodando",log=["$ do motor"])
  result=app._rodar(["/bin/sh","-c",script],cwd=repo,timeout=5)
  rows.append({"name":name,"script":script,"code":result.returncode,"stdout":result.stdout,"stderr":result.stderr,"state":{k:v for k,v in app.estado().items() if k!="ts"}})
 out.write_text(json.dumps({"source_commit":commit,"command":"python generate-command-log.py --reference <checkout> --output <fixture>","normalization":"Somente timestamp do estado; saída, código e últimas400linhas integrais.","cases":rows},ensure_ascii=False,indent=2)+"\n");print(f"{len(rows)} casos capturados")

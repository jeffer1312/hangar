"""Executa os snippets originais do shell em arquivos temporários."""
import argparse,json,os,subprocess,sys,tempfile
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);args=parser.parse_args()
source=subprocess.check_output(['git','show','30c3e5ba0^:scripts/hangar-send'],text=True)
start=source.index('"$PY" - "$PEERS_FILE" "$1" <<\'PY\'\n')+len('"$PY" - "$PEERS_FILE" "$1" <<\'PY\'\n');config=source[start:source.index('\nPY',start)]
start=source.index('peers_tsv=$("$PY" -c \'')+len('peers_tsv=$("$PY" -c \'');tsv=source[start:source.index('\' "$PEERS_FILE")',start)]
rows=[]
with tempfile.TemporaryDirectory(prefix='hangar-peer-cli-') as tmp:
 path=Path(tmp)/'peers.json'
 for field in ['token','base_url']:
  for value in ['NaN','Infinity','123456789012345678901234567890','true','[1]','{"x":1}','"\\ud800"','"açúcar"']:
   initial='{"a":{"'+('base_url' if field=='token' else 'token')+'":"'+('https://a.invalid' if field=='token' else 'fixture')+'","'+field+'":'+value+'}}'
   valid='"valid":{"base_url":"https://valid.invalid/","token":"fixture-valid"}'
   for position,document in [('single',initial),('before','{'+valid+','+initial[1:]),('after',initial[:-1]+','+valid+'}')]:
    path.write_text(document)
    for command,code in [('config',config),('tsv',tsv)]:
     result=subprocess.run([sys.executable,'-c',code,str(path),'a'],capture_output=True,env={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PYTHONIOENCODING':'utf-8:strict'})
     rows.append(dict(field=field,value=value,position=position,command=command,initial=document,rc=result.returncode,stdout_hex=result.stdout.hex(),stderr=result.stderr.decode().replace(str(path),'<FILE>')))
 for kind in ['unknown','active','disabled']:
  invalid=json.dumps({'\ud800':{'base_url':'https://a.invalid','token':'fixture',**({'enabled':False} if kind=='disabled' else {})}})
  valid='"valid":{"base_url":"https://valid.invalid/","token":"fixture-valid"}'
  for position,document in [('single',invalid),('before','{'+valid+','+invalid[1:]),('after',invalid[:-1]+','+valid+'}')]:
   path.write_text(document);command='config' if kind=='unknown' else 'tsv';code=config if command=='config' else tsv
   result=subprocess.run([sys.executable,'-c',code,str(path),'a'],capture_output=True,env={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PYTHONIOENCODING':'utf-8:strict'})
   rows.append(dict(field='id',value='"\\ud800"',kind=kind,position=position,command=command,initial=document,rc=result.returncode,stdout_hex=result.stdout.hex(),stderr=result.stderr.decode().replace(str(path),'<FILE>')))
args.output.write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','30c3e5ba0^'],text=True).strip(),command='uv run --project backend python backend/tests/fixtures/contract/peers/generate-cli-values.py --output backend/tests/fixtures/contract/peers/cli-values.json',cases=rows),ensure_ascii=True,indent=2)+'\n')
print(len(rows),'casos CLI capturados')

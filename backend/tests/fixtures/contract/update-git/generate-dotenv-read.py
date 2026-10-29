"""Caracteriza python-dotenv e a precedência efetiva de Settings sem ambiente pessoal."""
import argparse,io,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
cases=[('file-wins-interpolation','CHANNEL=file\nCP_SERVER_ID=${CHANNEL}\n',{'CHANNEL':'environment','CP_SERVER_ID':'final-environment'}),('env-interpolation','CP_SERVER_ID=${CHANNEL}\n',{'CHANNEL':'environment'}),('forward','CP_SERVER_ID=${CHANNEL}\nCHANNEL=later\n',{}),('defaults','EMPTY=\nUNSET\nA=${MISSING:-fallback}\nB=${EMPTY:-fallback}\nC=${UNSET:-fallback}\nCP_SERVER_ID=${MISSING:-fixture}\n',{}),('nested-default','A=${MISSING:-${OTHER:-fallback}}\nCP_SERVER_ID=fixture\n',{}),('escapes',r'''A="a\nb\tc\r\v\f\a\b\\\'\"\q"
B='a\nb\\c\'d'
CP_SERVER_ID=fixture
''',{}),('quoted-interpolation',"CHANNEL=fixture\nCP_SERVER_ID='${CHANNEL}'\n",{}),('multiline','A="line1\nline2"\nCP_SERVER_ID=fixture\n',{}),('invalid-lines','bad value\nBROKEN="unfinished\nCP_SERVER_ID=fixture\n',{}),('export-comment',"export 'CP_SERVER_ID' = fixture # comment\nA=abc#literal\n",{}),('case-insensitive','cp_server_id=fixture\n',{}),('duplicate','CP_SERVER_ID=first\nCP_SERVER_ID=second\n',{}),('unset-duplicate','A=first\nA\nCP_SERVER_ID=${A:-fallback}\n',{}),('unicode-whitespace','\u2003CP_SERVER_ID\u2003=\u2003fixture\u2003# tail\n',{}),('bom','\ufeffCP_SERVER_ID=fixture\n',{}),('bare-dollar','CP_SERVER_ID=$CHANNEL\n',{'CHANNEL':'value'})]
cases += [('bare-target','CP_SERVER_ID\n',{}),('case-collision','CP_SERVER_ID=first\ncp_server_id=second\nCP_SERVER_ID=third\n',{}),('crlf','A="first\r\nsecond"\r\nCP_SERVER_ID=fixture\r\n',{}),('quoted-trailing-invalid','A="first\nCP_SERVER_ID=ignored\nlast" invalid\nCP_SERVER_ID=fixture\n',{})]
cases += [(f'control-{code:02x}',f'CP_SERVER_ID{chr(code)}={chr(code)}fixture{chr(code)}# tail\nA=value{chr(code)}\n',{}) for code in range(0x1c,0x20)]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.clear();os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(ref/'backend'))
 from dotenv import dotenv_values
 from app.config import Settings
 rows=[]
 for name,raw,env in cases:
  os.environ.clear();os.environ.update(HOME=tmp,USERPROFILE=tmp,**env);path=Path(tmp)/'.env';path.write_text(raw)
  parsed=dict(dotenv_values(stream=io.StringIO(raw)))
  try:final=Settings(_env_file=path).server_id
  except Exception as exc:final={'error':type(exc).__name__}
  rows.append(dict(name=name,raw=raw,environment=env,parsed=parsed,server_id=final))
 out.write_text(json.dumps(dict(source_commit=commit,command='python backend/tests/fixtures/contract/update-git/generate-dotenv-read.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/dotenv-read.json',cases=rows),ensure_ascii=False,indent=2)+'\n');print(json.dumps(rows,ensure_ascii=False))

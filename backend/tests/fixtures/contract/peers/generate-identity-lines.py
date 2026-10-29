"""Captura os bytes do escritor legado de identidade, sem serviço ou configuração pessoal."""
import json,os,subprocess,sys,tempfile
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(reference/'backend'))
 from app import peers_api
 path=Path(tmp)/'.env';peers_api._ENV_ARQUIVO=path
 sources=['A=keep\r\nCP_SERVER_ID=old\r\n','\ufeffCP_SERVER_ID=old\n','export CP_SERVER_ID=old\n',' CP_SERVER_ID=old\n','A="first\nCP_SERVER_ID=inside\nlast"\n','CP_SERVER_ID=one\nCP_SERVER_ID=two','A=keep\u001cCP_SERVER_ID=old\u001dB=keep\u0085','A=keep\u2028CP_SERVER_ID=old\u2029']
 for raw in sources:
  for value in ['next','']:
   path.write_bytes(raw.encode());peers_api._escrever_id_env(value)
   rows.append(dict(raw=raw,value=value,output=path.read_bytes().decode()))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-identity-lines.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2)+'\n')
 print(f'{len(rows)} casos de escrita de identidade capturados')

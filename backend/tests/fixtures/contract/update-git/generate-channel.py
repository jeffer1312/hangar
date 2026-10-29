"""Captura edição de dotenv e validação do canal no Python, sem rede nem serviço."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

parser=argparse.ArgumentParser()
parser.add_argument('--reference',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
reference=args.reference.resolve();output=args.output.resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-channel-reference-') as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=str(Path(scratch)/'claude'),CP_AUTH_TOKEN='contract-owner')
    os.chdir(scratch);sys.path.insert(0,str(reference/'backend'))
    from app import update_channel
    from app.config import valid_update_branch
    update_channel.ENV_FILE=Path(scratch)/'.env'
    rows=[]
    cases=[('', 'empty'),('# config\r\nOTHER=value','crlf'),('\ufeffOTHER=value\r\n','bom'),
           ("export CP_UPDATE_BRANCH = 'old'\nCP_UPDATE_BRANCH=duplicate\nOTHER=x\n",'duplicates'),
           ('OTHER="first\r\nCP_UPDATE_BRANCH=inside-value\r\nlast"\r\n','multiline-other'),
           ('CP_UPDATE_BRANCH="old\nvalue" # after\nOTHER=x\n','multiline-target'),
           ("'CP_UPDATE_BRANCH'=old\n",'quoted-key'),('CP_UPDATE_BRANCH\n','no-value'),
           ('CP_UPDATE_BRANCH=old#value\n','hash-value'),('CP_UPDATE_BRANCH=old  \n','trailing-space'),
           ('CP_UPDATE_BRANCH="unterminated\nOTHER=x\n','malformed-quote')]
    for value in ['old',"'old'",'"old # inside"']:
        cases.append((f'OTHER=x\r\n\r\n  export CP_UPDATE_BRANCH = {value}  # keep this\r\nAFTER=y\r\n','spacing-'+value))
    cases += [(f'CP_UPDATE_BRANCH{chr(code)}={chr(code)}old{chr(code)}# tail\n', f'control-{code:02x}') for code in range(0x1c,0x20)]
    cases += [('\u2003CP_UPDATE_BRANCH\u2003=\u2003old\u2003# tail\n', 'unicode-space')]
    for raw,name in cases:
        update_channel.ENV_FILE.write_bytes(raw.encode())
        update_channel._write_env('test/channel','test/channel')
        rows.append({'name':name,'input':raw,'expected':update_channel.ENV_FILE.read_bytes().decode()})
    branches=['','main','beta','test/channel','../bad','-option','a b','a..b','x'*100,'x'*101,'a.b','a/../b','a//b','a/.b','á','0','/leading','trailing/','HEAD','latest']
    output.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-channel.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/channel-cases.json','dotenv':rows,'validation':[{'branch':value,'valid':valid_update_branch(value)} for value in branches]},ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} edições dotenv + {len(branches)} validações capturadas de {commit}')

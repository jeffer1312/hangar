"""Captura o executor Python em subprocessos isolados, sem backend ou serviço."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

parser=argparse.ArgumentParser()
parser.add_argument('--reference',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
reference=args.reference.resolve();output=args.output.resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as temp:
    os.environ['HOME']=temp;os.environ['USERPROFILE']=temp
    os.chdir(temp)
    sys.path.insert(0,str(reference/'backend'))
    from app import atualizar
    rows=[]
    for name,script in [('merged',"printf 'first\\n'; printf 'second\\n' >&2"),('ansi',"printf '\\033[31mred\\033[0m\\n'"),('nonzero',"printf 'failure\\n' >&2; exit 7"),('invalid-utf8',"printf '\\377\\n'"),('universal-newlines',"printf 'one\\rtwo\\r\\nthree\\n'"),('split-crlf',"printf 'one\\r'; sleep 0.05; printf '\\ntwo\\rthree\\n'")]:
        logs=[]
        result=atualizar._rodar(['sh','-c',script],cwd=Path(temp),timeout=3,log=logs.append)
        rows.append({'name':name,'script':script,'code':result.returncode,'stdout':result.stdout,'stderr':result.stderr,'logs':logs})
    script='sleep 0.9; exec 1>&- 2>&-; sleep 0.3'
    logs=[]
    result=atualizar._rodar(['sh','-c',script],cwd=Path(temp),timeout=1,log=logs.append)
    rows.append({'name':'post-eof-grace','script':script,'timeout':1,'code':result.returncode,'stdout':result.stdout,'stderr':result.stderr,'logs':logs})
    result=atualizar._rodar(['sh','-c','sleep 60 >/dev/null 2>&1 & echo $! > persistent.pid'],cwd=Path(temp),timeout=3,log=lambda _:None)
    persistent=int((Path(temp)/'persistent.pid').read_text())
    try:
        os.kill(persistent,0)
        rows.append({'name':'success-preserves-descendant','survived':True,'code':result.returncode})
    finally:
        os.kill(persistent,signal.SIGKILL)
    pidfile=Path(temp)/'child.pid'
    try:
        atualizar._rodar(['sh','-c','sleep 60 & echo $! > child.pid; wait'],cwd=Path(temp),timeout=.1,log=lambda _:None)
    except subprocess.TimeoutExpired:
        pid=int(pidfile.read_text());survived=False
        try:
            os.kill(pid,0);survived=True
        except ProcessLookupError:
            pass
        finally:
            if survived:os.kill(pid,signal.SIGKILL)
        rows.append({'name':'timeout-child','child_survived':survived})
    output.write_text(json.dumps({'source_commit':commit,'command':'python generate-process.py --reference <checkout> --output process-cases.json','cases':rows},ensure_ascii=False,indent=2)+'\n')
    print('RED: filho sobreviveu ao timeout Python =',survived)
    assert not survived, 'PEE-072: o prazo precisa encerrar também o filho criado pelo comando'

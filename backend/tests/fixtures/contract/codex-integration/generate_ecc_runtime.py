"""Captura o preparo ECC original com subprocessos controlados e arquivos isolados."""
import ast
import json
import logging
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
def extract(path,names,namespace):
    source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
    nodes=[n for n in ast.parse(source).body if isinstance(n,ast.FunctionDef) and n.name in names]
    exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),namespace)
ns=dict(Path=Path,json=json,re=re,shutil=shutil,subprocess=subprocess,_log=logging.getLogger('fixture'))
extract('backend/app/codex_arquivos.py',{'ler','json_obj'},ns)
extract('backend/app/codex_compat.py',{'_preparar_runtime_ecc'},ns)
cases=[]
scenarios=[('missing',{}),('empty',{'dependencies':{}}),('null',{'dependencies':None}),('list',{'dependencies':['a']}),
 ('name',{'dependencies':{'../a':'1.0.0'}}),('uppercase',{'dependencies':{'Name':'1.0.0'}}),
 ('floating',{'dependencies':{'a':'^1.0.0'}}),('healthy',{'dependencies':{'a':'1.0.0'}}),
 ('install',{'dependencies':{'a':'1.0.0'}}),('failure',{'dependencies':{'a':'1.0.0'}}),
 ('timeout',{'dependencies':{'a':'1.0.0'}}),('unhealthy-after',{'dependencies':{'a':'1.0.0'}}),
 ('no-node',{'dependencies':{'a':'1.0.0'}}),('no-npm',{'dependencies':{'a':'1.0.0'}}),
 ('no-cli-js',{'dependencies':{'a':'1.0.0'}}),('locked',{'dependencies':{'a':'^1.0.0'}}),
 ('scope',{'dependencies':{'@scope/name':'1.2.3-beta'}}),('unicode-version',{'dependencies':{'a':'١.٢.٣'}})]
for name,package in scenarios:
    with tempfile.TemporaryDirectory() as raw:
        root=Path(raw);plugin=root/'plugin';plugin.mkdir();backups=root/'backups';calls=[]
        (plugin/'package.json').write_text(json.dumps(package))
        if name=='locked':(plugin/'package-lock.json').write_text(json.dumps({'packages':{'node_modules/a':{'version':'1.0.0'}}}))
        npm=root/('npm' if name=='no-cli-js' else 'npm-cli.js');npm.write_text('fixture')
        def install_versions():
            for key,version in package.get('dependencies',{}).items():
                target=plugin/'node_modules'/key/'package.json';target.parent.mkdir(parents=True,exist_ok=True)
                target.write_text(json.dumps({'version':'1.0.0' if name=='locked' else version}))
        if name=='healthy':install_versions()
        def run(args,**kwargs):
            calls.append(dict(args=args,timeout=kwargs.get('timeout'),cwd=str(kwargs['cwd']) if kwargs.get('cwd') else None,stdin=kwargs.get('stdin')))
            if '-e' in args:return subprocess.CompletedProcess(args,1 if name=='unhealthy-after' else 0,stdout=b'',stderr=b'')
            if name=='timeout':raise subprocess.TimeoutExpired(args,120,output=b'fixture stdout',stderr=b'fixture stderr')
            if name!='failure':install_versions()
            return subprocess.CompletedProcess(args,1 if name=='failure' else 0,stdout=b'fixture stdout',stderr=b'fixture stderr')
        def write(path,data,expected):
            path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
        ns['gravar']=write
        with patch.object(shutil,'which',side_effect=lambda program:None if name=='no-'+program else str(npm if program=='npm' else root/'node')),patch.object(subprocess,'run',run):
            try:result=dict(ok=ns['_preparar_runtime_ecc'](plugin,backups))
            except Exception as error:result=dict(error=type(error).__name__,message=str(error))
        log=backups/'ecc-runtime-npm.log'
        case=dict(name=name,package=package,result=result,calls=calls,diagnostic_hex=log.read_bytes().hex() if log.exists() else None)
        cases.append(json.loads(json.dumps(case).replace(raw,'$ROOT')))
Path(__file__).with_name('ecc-runtime.json').write_text(json.dumps(dict(source_commit=COMMIT,
 command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_ecc_runtime.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))

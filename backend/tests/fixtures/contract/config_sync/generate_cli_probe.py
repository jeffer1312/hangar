"""Executa as sondas originais com saídas de CLI e relógio controlados."""
import ast
import json
import logging
from pathlib import Path
import re
import subprocess
from types import SimpleNamespace

BACKEND=Path(__file__).resolve().parents[4]
names={'cli_version','mods_by_default','aceita_plugin_dir','esquecer_capacidade','_versao','esquecer_versao'}
nodes=[]
for file in ['plugin_bridge.py','harness_saude.py']:
    tree=ast.parse((BACKEND/'app'/file).read_text())
    nodes.extend(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in names)
code=compile(ast.Module(body=nodes,type_ignores=[]),'captura-sondas','exec')
cases=[]
for name,stdout,stderr,present,fail in [
    ('modern','2.1.287 (Claude Code)\n','',True,False),('old','2.1.286\n','',True,False),
    ('leading','  3.0.0\nsegunda','',True,False),('prefix','v2.1.287\n','',True,False),
    ('stderr','','2.2.0\nsegunda',True,False),('empty','','',True,False),
    ('huge','999999999999999999999999.0.0','',True,False),('unicode','٢.١.٢٨٧','',True,False),
    ('absent','','',False,False),('failure','','',True,True),
]:
    for supported in [False,True]:
        calls=[]
        def run(args,**kwargs):
            calls.append({'args':args,'timeout':kwargs['timeout']})
            if fail: raise subprocess.TimeoutExpired(args,kwargs['timeout'])
            out=('usage --plugin-dir path' if supported else 'usage') if args[-1]=='--help' else stdout
            return SimpleNamespace(stdout=out,stderr=stderr,returncode=0)
        ns={'time':SimpleNamespace(monotonic=lambda:1000),'shutil':SimpleNamespace(which=lambda cli:'/fixture/'+cli if present else None),
            'subprocess':SimpleNamespace(run=run,SubprocessError=subprocess.SubprocessError), 're':re,'_log':logging.getLogger('fixture'),
            '_versao':None,'_versoes':{},'_capacidade':None,'_TTL_CAPACIDADE_S':600,'MODS_BY_DEFAULT':(2,1,287)}
        # A função da saúde e o cache do plugin têm o mesmo nome em módulos diferentes.
        plugin_nodes=[n for n in nodes if n.name not in {'_versao','esquecer_versao'}]
        exec(compile(ast.Module(body=plugin_nodes,type_ignores=[]),'plugin','exec'),ns)
        version=ns['cli_version']();mods=ns['mods_by_default']();help_ok=ns['aceita_plugin_dir']()
        health={**ns,'_versoes':{}}
        exec(compile(ast.Module(body=[n for n in nodes if n.name in {'_versao','esquecer_versao'}],type_ignores=[]),'saude','exec'),health)
        health_version=health['_versao']('claude')
        cases.append({'name':name,'stdout':stdout,'stderr':stderr,'present':present,'failure':fail,'supported':supported,
                      'version':version,'mods':mods,'help':help_ok,'health':health_version,'calls':calls})
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/config_sync/generate_cli_probe.py','cases':cases}
Path(__file__).with_name('cli-probe.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')

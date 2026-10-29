"""Captura descoberta de provedores sem executar CLIs nem consultar o PATH pessoal."""
import json
import ntpath
import posixpath
import subprocess
import types
from pathlib import Path
from unittest.mock import patch
from app import cli_probe
cases = []
for windows in (False, True):
    paths = [r'C:\fixture\one', r'C:\fixture\two'] if windows else ['/fixture/one', '/fixture/two']
    pathmod = ntpath if windows else posixpath
    for scenario in ['empty', 'executable', 'denied', 'later', 'extensionless', 'non_cli']:
        extension = '.CMD' if windows and scenario not in ('extensionless', 'non_cli') else '.JS' if windows and scenario == 'non_cli' else ''
        files = {} if scenario == 'empty' else {pathmod.join(paths[0], 'claude' + extension): scenario not in ('denied', 'later')}
        if scenario == 'later':
            files[pathmod.join(paths[1], 'claude' + extension)] = True
        calls = []
        fake_os = types.SimpleNamespace(name='nt' if windows else 'posix', pathsep=';' if windows else ':',
            environ={'PATHEXT':'.JS;.EXE;.CMD;.BAT;.COM'}, X_OK=1,
            path=types.SimpleNamespace(join=pathmod.join, isfile=lambda p: calls.append(p) or p in files),
            access=lambda p, mode: files.get(p, False))
        with patch.object(cli_probe, 'os', fake_os), patch.object(cli_probe, '_obter_path', return_value=fake_os.pathsep.join(paths)), \
             patch.object(cli_probe.subprocess, 'run', side_effect=AssertionError('a descoberta não executa CLIs')):
            result = cli_probe._sondar_sem_cache()
            first_calls = len(calls)
            cached = cli_probe.sondar_providers()
            cases.append({'windows':windows,'scenario':scenario,'paths':paths,'files':files,
                          'pathext':fake_os.environ['PATHEXT'],'result':result,
                          'cache_same':cached == result,'cache_new_reads':len(calls)-first_calls})
login = []
for scenario, code, stdout in [('success',0,' /fixture/bin \n'),('empty',0,''),('nonzero',1,'/wrong'),('error',None,'')]:
    fake_os = types.SimpleNamespace(name='posix', environ={'SHELL':'/fixture/shell','PATH':'/fallback'})
    outcome = OSError('fixture') if code is None else types.SimpleNamespace(returncode=code,stdout=stdout)
    with patch.object(cli_probe,'os',fake_os), patch.object(cli_probe,'_path_cache',None), \
         patch.object(cli_probe,'_path_login',None), patch.object(cli_probe.subprocess,'run',side_effect=outcome if isinstance(outcome,Exception) else None,return_value=outcome) as run:
        value = cli_probe._obter_path()
        second = cli_probe._obter_path()
        login.append({'scenario':scenario,'code':code,'stdout':stdout,'result':value,
                      'second':second,'calls':run.call_count,'argv':run.call_args.args[0],
                      'timeout':run.call_args.kwargs['timeout']})
Path(__file__).with_name('cli-probe.json').write_text(json.dumps({
    'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
    'command':'PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_cli_probe.py',
    'cases':cases,'login':login},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')

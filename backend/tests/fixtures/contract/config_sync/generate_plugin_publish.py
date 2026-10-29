"""Captura arquivos do plugin e a ordem de publicação em falha, sem subir servidor."""
import ast
import hashlib
import hmac
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

BACKEND=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(BACKEND))
from app import atomico, config
from pytest import MonkeyPatch

names={'machine_key','machine_file','plugin_dir_file','raizes_dos_plugins','_publish_address','_publish_plugin_dir','_publish_for_machine'}
tree=ast.parse((BACKEND/'app/plugin_bridge.py').read_text())
code=compile(ast.Module(body=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in names],type_ignores=[]),'plugin-publication','exec')
cases=[]
for name,enabled,broken in [('enabled',True,False),('disabled',False,False),('address_failure',True,True)]:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(os.environ,{'HOME':folder,'USERPROFILE':folder},clear=True):
        home=Path(folder);repo=home/'repo';plugins=repo/'plugins'
        for plugin in ['zeta','hangar','alfa']:
            path=plugins/plugin/'.claude-plugin/plugin.json';path.parent.mkdir(parents=True);path.write_text('{}')
        (plugins/'sem-manifesto').mkdir()
        (home/'.hangar').mkdir()
        (home/'.hangar/plugin-dir').write_text('antigo\n')
        if broken:(home/'.hangar/plugin.json').mkdir()
        mp.setattr(config,'settings',config.Settings(_env_file=None,auth_token='fixture-owner',port=8765,projects_dir=home/'.claude/projects',sync_data=home/'.hangar/sync-vault.json'))
        ns={'Path':Path,'hmac':hmac,'hashlib':hashlib,'json':json,'atomico':atomico,'PLUGINS_ROOT':plugins,'PLUGIN_SRC':plugins/'hangar','ligado':lambda:enabled,'_log':logging.getLogger('fixture')}
        exec(code,ns)
        try:
            ns['_publish_for_machine'](home);result={'ok':True}
        except Exception as error:
            result={'error':type(error).__name__}
        files={str(p.relative_to(home)).replace('\\','/'):p.read_text().replace(folder,'⟦HOME⟧') for p in home.rglob('*') if p.is_file() and '.hangar' in p.parts}
        cases.append({'name':name,'enabled':enabled,'broken':broken,'result':result,'files':files})
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/config_sync/generate_plugin_publish.py','cases':cases}
Path(__file__).with_name('plugin-publish.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')

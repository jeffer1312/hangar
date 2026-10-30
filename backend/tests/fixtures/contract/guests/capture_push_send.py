"""Caracteriza payload e poda originais, substituindo somente o transporte Web Push."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch)
    from app import push
    from pywebpush import WebPushException
    push.settings.vapid_private='fixture-private'
    push.settings.vapid_subject='mailto:fixture@example.invalid'
    base={'subscription':{'endpoint':'https://fixture.invalid/e'},'label':'Casa','serverId':'s1','locale':'en'}
    cases=[(base, {})]
    for key in ['label','serverId','locale']:
        for value in [None,False,0,1,[],{},[1],{'a':1},'','é','\ud800']:
            cases.append((base|{key:value},{}))
        cases.append(({k:v for k,v in base.items() if k!=key},{}))
    cases += [(None,{}),([],{}),(1,{})]
    cases += [(base,{'tag':'aggregate','title_suffix_fn':lambda locale:'2 sessões','url':'/'})]
    rows=[]
    for entry, options in cases:
        calls=[]
        def send(**kwargs): calls.append(kwargs)
        try:
            with patch('pywebpush.webpush',send):
                result={'alive':push._send_one(entry,'sessão',lambda locale:push._msg(locale,'aguardando'),**options)}
        except Exception as error: result={'error':type(error).__name__}
        rows.append({'entry':entry,'aggregate':bool(options),'calls':calls,**result})
    statuses=[]
    for status in [200,201,400,401,404,410,429,500,503,None]:
        def send(**kwargs):
            if status not in [200,201]:
                raise WebPushException('fixture',response=SimpleNamespace(status_code=status))
        with patch('pywebpush.webpush',send):
            alive=push._send_one(base,'s',lambda locale:'texto')
        statuses.append({'status':status,'alive':alive})
    output=dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
                command='cd backend && uv run python tests/fixtures/contract/guests/capture_push_send.py',
                isolation='push._send_one e _msg reais, somente pywebpush.webpush substituído; sem rede, chaves fictícias, HOME temporário. Não prova criptografia nem aceitação de provedor real.',rows=rows,statuses=statuses)
    Path(__file__).with_name('push-send.json').write_text(json.dumps(output,ensure_ascii=True,indent=2)+'\n')
    print(len(rows),len(statuses))

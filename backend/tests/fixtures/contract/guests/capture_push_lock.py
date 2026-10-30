"""Caracteriza reinscrição concorrente à poda original com barreiras determinísticas."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from unittest.mock import patch

sys.path.insert(0,str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch)
    from app import push
    push.settings.projects_dir=Path(scratch)/'projects'
    push.settings.vapid_private='fixture-private'
    push.settings.vapid_public='fixture-public'
    rows=[]
    for changed in [False,True]:
        push._save([])
        old={'endpoint':'https://fixture.invalid/e','keys':{'auth':'old'}}
        new=old|{'keys':{'auth':'new'}} if changed else old.copy()
        push.add_subscription(old,'Servidor','s')
        sending=threading.Event();release=threading.Event();attempted=threading.Event();finished=threading.Event()
        lock=threading.Lock()
        class ObservedLock:
            def __enter__(self):
                if threading.current_thread().name=='subscriber': attempted.set()
                lock.acquire()
            def __exit__(self,*args): lock.release()
        def send(*args,**kwargs):
            sending.set()
            assert release.wait(5)
            return False
        def subscribe():
            push.add_subscription(new,'Servidor','s')
            finished.set()
        with patch.object(push,'_lock',ObservedLock()), patch.object(push,'_send_one',send), patch.object(push,'_to_expo',lambda *args:None):
            first=threading.Thread(target=push._broadcast,args=('s',lambda locale:'texto'),name='broadcast')
            first.start();assert sending.wait(5)
            second=threading.Thread(target=subscribe,name='subscriber')
            second.start();assert attempted.wait(5)
            blocked=not finished.is_set()
            release.set();first.join(5);second.join(5)
            assert not first.is_alive() and not second.is_alive()
        rows.append(dict(changed=changed,blocked=blocked,final=push._load()))
    output=dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
                command='cd backend && uv run python tests/fixtures/contract/guests/capture_push_lock.py',
                isolation='Funções originais e arquivos temporários; transporte substituído por barreiras, trava instrumentada conservando exclusão. Reinscrição aguarda antes da poda, inclusive com conteúdo idêntico.',rows=rows)
    Path(__file__).with_name('push-lock.json').write_text(json.dumps(output,ensure_ascii=True,indent=2)+'\n')
    print(len(rows))

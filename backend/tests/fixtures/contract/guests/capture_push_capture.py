"""Executa capture_pane/_run originais com processo temporário; sem multiplexador ou backend."""
import json,os,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0,str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch)
    from app import tmux,agentpane
    rows=[]
    cases=[('zero',b'pergunta\n',0,None),('nonzero',b'pergunta\n',7,None),('empty',b'',7,None),('invalid',b'a\xffb\n',0,None),('newlines',b'a\r\nb\rc\n',0,None),('missing',b'',0,'missing'),('timeout',b'',0,'timeout')]
    for name in ['fixture','0']:
        for target in [None,'%7']:
            for label,raw,status,error in cases:
                calls=[]
                def run(args,**kwargs):
                    calls.append(args)
                    if error=='missing':raise FileNotFoundError('fixture')
                    if error=='timeout':raise subprocess.TimeoutExpired('fixture',5)
                    return subprocess.run([sys.executable,'-c',f'import os;os.write(1,bytes.fromhex("{raw.hex()}"));raise SystemExit({status})'],**kwargs)
                with patch.object(agentpane,'resolve_target',lambda _:target),patch.object(tmux,'RUN',run):
                    value=tmux.capture_pane(name)
                rows.append({'name':name,'target':target,'case':label,'stdout_hex':raw.hex(),'exit':status,'error':error,'result':value,'args':calls[0]})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_push_capture.py','isolation':'capture_pane e _run originais; resolve_target controlado; subprocesso Python temporário substitui tmux. Timeout/FileNotFoundError injetados na fronteira subprocess.run; demais saídas reais.','rows':rows}
    (Path(__file__).resolve().parent/'push-capture.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(len(rows))

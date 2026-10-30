"""Caracteriza o produtor original sem multiplexador, processo ou foco reais."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch
sys.path.insert(0, str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch, SHELL='/fixture/shell')
    from app import shortcut_terminals as terminal, tmux
    directory = Path(scratch)
    rows = []
    for windows in [False, True]:
        for failure in ['', 'new-session', '@cp_hidden', '@cp_shortcut_key']:
            calls = []
            def run(args, **kwargs):
                calls.append(args)
                return subprocess.CompletedProcess(args, 1 if failure and failure in args else 0, '', '')
            for path in directory.iterdir():
                if path.is_file():
                    path.unlink()
            with patch.object(terminal, '_IS_WINDOWS', windows), patch.object(terminal, '_OEM', 'latin-1'), \
                 patch.object(terminal, '_windows_dir', return_value=directory), \
                 patch.object(terminal.secrets, 'token_hex', return_value='abcdef'), \
                 patch.object(terminal.time, 'time_ns', return_value=2000000000123456789), \
                 patch.object(tmux, '_scope_prefix', return_value=['fixture-scope', '--']), \
                 patch.object(tmux, '_run', side_effect=run), \
                 patch.object(tmux, 'has_session', return_value=False), \
                 patch.object(tmux, 'kill_session', side_effect=lambda name: calls.append(['kill', name]) or True):
                result = terminal.start('owner.a:b', '/fixture/project', 'echo a;\necho b', '  Rotulo\tX; ',
                                        {'DISPLAY': ':fixture'}, key=' global:k; ', ask=False)
            rows.append(dict(windows=windows, failure=failure, result=result, calls=calls,
                             files={p.name: p.read_bytes().replace(scratch.encode(), b'<DIR>').hex()
                                    for p in directory.iterdir() if p.is_file()}))
    statuses=[]
    for code, raw in [(0,'1\t\t\n'),(0,'1\t0\t\n'),(0,'1\t3\t\n'),(0,'1\t\tHUP\n'),(0,'0\t\t\n'),(1,'1\t3\t\n')]:
        with patch.object(terminal, '_IS_WINDOWS', False), patch.object(tmux, '_run', return_value=subprocess.CompletedProcess([],code,raw,'')):
            statuses.append(dict(code=code, raw=raw, result=terminal.status('shortcut-fixture-abcdef')))
    escaping=[]
    with patch.dict(os.environ, {'HANGAR_TEST_VAR':'1'}, clear=True):
        for raw in ['start https://x/a%20b','for %i in (a) do echo %i','echo %1 %* %~dp0','echo 100%%',
                    'echo %HANGAR_TEST_VAR% %NAO_EXISTE_XYZ%','echo %ERRORLEVEL% %cd%','echo %VAR:~0,3%']:
            escaped=terminal._batch_escape(raw)
            escaping.append(dict(raw=raw, escaped=escaped, restored=terminal._batch_unescape(escaped)))
    started=[]
    def launch(*args, **kwargs):
        started.append((args, kwargs))
        return {'id': 'abcdef', 'label': 'Rotulo', 'tmux':'shortcut-hangar-abcdef'}
    def read_rows():
        return [] if not started else [{'owner':'','key':'global:k','alive':True,'id':'abcdef','label':'Rotulo','tmux':'shortcut-hangar-abcdef'}]
    with patch.object(terminal,'start',side_effect=launch), patch.object(terminal,'_read_rows',side_effect=read_rows):
        with ThreadPoolExecutor(4) as pool:
            results=list(pool.map(lambda _: terminal.start_hangar(' global:k ', '/fixture/project', 'echo x', 'Rotulo', {}, 's', True), range(4)))
    concurrent={'started':len(started),'reused':sorted(reused for _,reused in results),'ids':sorted({term['id'] for term,_ in results})}
    data={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
          'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_shortcut_terminal.py',
          'isolation':'Funções originais; tmux e foco não executados, saída simulada; Windows é ramo simulado em Linux com latin-1, não prova OEM/psmux real; IDs/relógio fixos.',
          'start':rows,'status':statuses,'batch':escaping,'concurrent':concurrent}
    text=json.dumps(data,ensure_ascii=False,indent=2).replace(scratch,'<DIR>')
    Path('tests/fixtures/contract/guests/shortcut-terminal.json').write_text(text+'\n',encoding='utf-8')
    print(f'{len(rows)} criações, {len(statuses)} estados, {len(escaping)} escapes e concorrência capturados')

"""Contratos dos handlers originais de atalhos com efeitos do multiplexador isolados."""
import ast
import contextlib
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

from fastapi import Depends, FastAPI, HTTPException, Request
from fastapi.testclient import TestClient

sys.path.insert(0, str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch,
                      CP_AUTH_TOKEN='owner', CP_LAN_BIND_IP='127.0.0.1')
    os.environ.pop('HANGAR_AUTH_CONTEXT', None)
    from app import guest_users, project_shortcuts, shortcut_terminals, terminal_prompt, termsock
    from app.mensagens import erro
    from app.models import ShortcutShellBody, RunCodeBody, ShortcutAnswerBody
    root = Path(scratch) / 'allowed'
    root.mkdir()
    app = FastAPI()
    def auth():
        pass
    names = {'shortcut_shell', 'run_code', '_shortcut_shell', '_shortcut_started',
             '_shortcut_output_tail', '_shortcut_shell_hangar', '_shortcut_reused',
             '_shortcut_mux_unavailable', '_answer', 'shortcut_terminal_answer',
             'shortcut_terminal_close', 'hangar_terminal_answer', 'hangar_terminal_close',
             'hangar_terminal_restart', '_hangar_404'}
    nodes = [n for n in ast.parse(Path('app/api.py').read_text()).body
             if isinstance(n, ast.FunctionDef) and n.name in names]
    calls = []
    tick = [0.0]
    def monotonic():
        tick[0] += 1.0
        return tick[0]
    ns = dict(app=app, Depends=Depends, require_auth=auth, _transfer_guard=auth,
              HTTPException=HTTPException, Request=Request, erro=erro,
              ShortcutShellBody=ShortcutShellBody, RunCodeBody=RunCodeBody,
              ShortcutAnswerBody=ShortcutAnswerBody, guest_users=guest_users,
              project_shortcuts=project_shortcuts,
              os=SimpleNamespace(name='posix', path=os.path),
              shutil=SimpleNamespace(which=lambda name: '/fixture/' + name),
              _session_cwd=lambda name: str(root), _shortcut_display_env=lambda: {},
              _focus_hangar_terminal=lambda row: False,
              _SHORTCUT_FAIL_WINDOW=2.0,
              time=SimpleNamespace(monotonic=monotonic, sleep=lambda seconds: calls.append(['sleep', seconds])),
              _log=logging.getLogger('shortcut-contract'))
    exec(compile(ast.Module(body=nodes, type_ignores=[]), 'app/api.py', 'exec'), ns)
    term = {'id': 'abcdef', 'label': 'Executar', 'tmux': 'shortcut-s-abcdef'}
    def start(*args, **kwargs):
        calls.append(['start', args, kwargs])
        return term.copy()
    cases = []
    shell_route = '/api/sessions/s/shortcut-shell'
    run_route = '/api/sessions/s/run-code'
    for route in [shell_route, run_route]:
        for body in [None, {}, [], {'command': 1}, {'command': ''}, {'command': ' \n '},
                     {'command': 'echo ok'}, {'command': '\x00'},
                     {'command': '⟦SEGREDO:fixture.key⟧'},
                     {'command': 'echo ok', 'key': 'bad'},
                     {'command': 'echo ok', 'key': 'run-code:a-b:1'},
                     {'command': 'echo ok', 'key': 'run-code:á'},
                     {'command': 'echo ok', 'language': 'python'},
                     {'command': 'echo ok', 'language': ' BASH '},
                     {'command': 'echo ok', 'language': 'pwsh'},
                     {'command': 'echo ok', 'runs_in': 'bad'},
                     {'command': 'echo ok', 'runs_in': 'hangar', 'key': 'key'}]:
            cases.append({'route': route, 'body': body, 'mode': 'alive'})
    for mode in ['success', 'failed', 'unknown', 'start_failed', 'shell_missing', 'panel_missing', 'windows']:
        cases.append({'route': run_route, 'body': {'command': 'echo ok', 'language': 'bash'}, 'mode': mode})
    for body in [{'command': 'x' * 4097}, {'command': 'echo ok', 'pasta': '/missing'},
                 {'command': 'echo ok', 'runs_in': 'hangar'},
                 {'command': 'echo ok', 'runs_in': 'hangar', 'key': 'key', 'home': False}]:
        cases.append({'route': run_route if len(body['command']) > 4096 else shell_route,
                      'body': body, 'mode': 'alive'})
    for route in ['/api/sessions/s/shortcut-terminals/abcdef', '/api/hangar-terminals/abcdef']:
        for text in ['', 'yes', 'á', 'a\nb', '\x00', '\x7f']:
            cases.append({'route': route + '/answer', 'body': {'text': text}, 'mode': 'alive'})
        for mode in ['missing', 'answer_failed']:
            cases.append({'route': route + '/answer', 'body': {'text': 'yes'}, 'mode': mode})
        for mode in ['alive', 'missing', 'close_failed']:
            cases.append({'route': route + '/close', 'body': None, 'mode': mode})
    rows = []
    with TestClient(app, raise_server_exceptions=False) as client:
        for case in cases:
            calls.clear()
            tick[0] = 0.0
            mode = case['mode']
            ns['os'].name = 'nt' if mode == 'windows' else 'posix'
            ns['shutil'].which = lambda name: None if mode == 'shell_missing' else '/fixture/' + name
            state = {'success': (False, 0), 'failed': (False, 3), 'unknown': (False, None)}.get(mode, (True, None))
            with contextlib.ExitStack() as stack:
                for target, value in [
                    ('app.termsock.painel_disponivel', lambda: mode != 'panel_missing'),
                    ('app.shortcut_terminals.start', lambda *a, **kw: None if mode == 'start_failed' else start(*a, **kw)),
                    ('app.shortcut_terminals.start_hangar', lambda *a, **kw: (start(*a, **kw), False)),
                    ('app.shortcut_terminals.status', lambda _: state),
                    ('app.shortcut_terminals.output', lambda _: 'linha 1\nlinha 2\nlinha 3\nlinha 4\nlinha 5\nlinha 6\n'),
                    ('app.shortcut_terminals.find', lambda *a: None if mode == 'missing' else term['tmux']),
                    ('app.shortcut_terminals.find_hangar', lambda *a: None if mode == 'missing' else term['tmux']),
                    ('app.terminal_prompt.answer', lambda *a: calls.append(['answer', a]) is None and mode != 'answer_failed'),
                    ('app.shortcut_terminals.close', lambda *a: None if mode == 'missing' else mode != 'close_failed'),
                    ('app.shortcut_terminals.close_hangar', lambda *a: None if mode == 'missing' else mode != 'close_failed')]:
                    stack.enter_context(patch(target, value))
                response = client.post(case['route'], json=case['body'])
                try:
                    body = response.json()
                except ValueError:
                    body = response.text
                rows.append(case | {'status': response.status_code, 'response': body, 'calls': list(calls)})
    result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'command': 'cd backend && uv run python tests/fixtures/contract/guests/capture_shortcut_http.py',
              'isolation': 'Handlers e modelos originais; autenticação e transferência substituídas por dependências vazias; cwd temporário; multiplexador, foco, disponibilidade e relógio injetados; sem lifespan nem processos reais. Windows é ramo simulado no Linux.',
              'rows': rows}
    output = json.dumps(result, ensure_ascii=True, indent=2).replace(scratch, '⟦ROOT⟧')
    Path('tests/fixtures/contract/guests/shortcut-http.json').write_text(output + '\n', encoding='utf-8')
    print(f'{len(rows)} respostas HTTP originais capturadas')

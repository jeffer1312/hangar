"""Bordas HTTP originais do scanner, com porteiro e disco/Git temporários."""
import ast
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from urllib.parse import urlencode
from fastapi import Depends, FastAPI, HTTPException
from fastapi.testclient import TestClient
sys.path.insert(0, str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    home = Path(scratch)
    root = home / 'allowed'
    project = root / 'project'
    project.mkdir(parents=True)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch,
                      CP_AUTH_TOKEN='owner', CP_LAN_BIND_IP='127.0.0.1',
                      GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1')
    os.environ.pop('HANGAR_AUTH_CONTEXT', None)
    subprocess.run(['git', '-C', str(project), 'init', '-q', '-b', 'main'], check=True)
    os.utime(project, (2000000000, 2000000000))
    (home / 'outside').mkdir()
    from app import fs, guest_users, git_ops
    from app.auth import require_auth, reset_backoff
    from app.guest_user_gate import GuestUserGate
    from app.mensagens import erro
    guest = guest_users.Guest('fixture', 'Ana', 'hash', str(root))
    guest_users.lookup_token = lambda token: guest if token == 'guest' else None
    app = FastAPI()
    app.add_middleware(GuestUserGate)
    names = {'fs_roots', 'fs_scan', 'fs_branches'}
    nodes = [n for n in ast.parse(Path('app/api.py').read_text()).body
             if isinstance(n, ast.FunctionDef) and n.name in names]
    ns = dict(app=app, Depends=Depends, require_auth=require_auth, HTTPException=HTTPException,
              list_roots=fs.list_roots.__wrapped__, scan_dir=fs.scan_dir.__wrapped__,
              list_branches=git_ops.list_branches.__wrapped__, FsError=fs.FsError,
              GitError=git_ops.GitError, erro=erro, Path=Path, os=os)
    exec(compile(ast.Module(body=nodes, type_ignores=[]), 'app/api.py', 'exec'), ns)
    cases = []
    for route in ['roots', 'scan', 'branches']:
        for query in ['', 'root=', urlencode({'root': root}),
                      urlencode({'root': root, 'path': project}),
                      urlencode({'root': root, 'path': ''}),
                      urlencode({'root': root, 'path': home / 'outside'}),
                      urlencode({'root': home / 'outside'}),
                      urlencode({'root': root, 'path': root / 'missing'})]:
            cases.append(dict(method='GET', path=f'/api/fs/{route}', query=query, body=''))
        query = urlencode({'root': root, 'path': project})
        cases.extend([
            dict(method='GET', path=f'/api/%66s/{route}', query=query, body=''),
            dict(method='GET', path=f'/api/fs/{route}', query=query, body='{'),
            dict(method='HEAD', path=f'/api/fs/{route}', query=query, body=''),
            dict(method='POST', path=f'/api/fs/{route}', query=query, body=''),
            dict(method='GET', path=f'/api/fs/{route}/', query=query, body='')])
    rows = []
    with TestClient(app, raise_server_exceptions=False) as client:
        for case in cases:
            reset_backoff()
            r = client.request(case['method'], case['path'] + '?' + case['query'],
                               headers={'authorization': 'Bearer guest', 'content-type': 'application/json'},
                               content=case['body'], follow_redirects=False)
            rows.append(case | {'status': r.status_code, 'response': r.json() if r.content else None})
    result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'command': 'cd backend && uv run python tests/fixtures/contract/guests/capture_fs_http.py',
              'isolation': 'Handlers e porteiro originais; lookup fixo; scanner e Git originais sem bridges, diretórios temporários; sem lifespan.',
              'rows': rows}
    output = json.dumps(result, ensure_ascii=False, indent=2).replace(scratch, '⟦ROOT⟧')
    output = output.replace(urlencode({'x': scratch})[2:], '⟦URLROOT⟧')
    Path('tests/fixtures/contract/guests/fs-http.json').write_text(output + '\n', encoding='utf-8')
    print(f'{len(rows)} respostas HTTP originais capturadas')

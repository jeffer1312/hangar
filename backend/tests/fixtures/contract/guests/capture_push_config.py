"""Caracteriza caminho e snapshot do push original sem escrever fora do temporário."""
import ast
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch,
                      CP_VAPID_PUBLIC='original-public', CP_VAPID_PRIVATE='original-private',
                      CP_VAPID_SUBJECT='mailto:fixture@example.invalid')
    from app.config import Settings
    nodes = [node for node in ast.parse(Path('app/push.py').read_text()).body
             if isinstance(node, ast.FunctionDef) and node.name == '_file']
    rows = []
    for raw in ['', '.', './projects', 'projects', 'one/projects', 'one/../projects',
                '~', '~/projects', '/', '/projects', '/fixture//projects/./', ' a/projects ']:
        settings = Settings(_env_file=None, projects_dir=raw)
        ns = dict(Path=Path, settings=settings)
        exec(compile(ast.Module(body=nodes, type_ignores=[]), 'app/push.py', 'exec'), ns)
        with patch.object(Path, 'mkdir'):
            result = str(ns['_file']())
        rows.append(dict(raw=raw, projects=str(settings.projects_dir), file=result))
    settings = Settings(_env_file=None)
    os.environ.update(CP_VAPID_PUBLIC='changed-public', CP_VAPID_PRIVATE='changed-private',
                      CP_VAPID_SUBJECT='mailto:changed@example.invalid')
    snapshot = {key: getattr(settings, key) for key in ['vapid_public', 'vapid_private', 'vapid_subject']}
    for key in ['CP_VAPID_PUBLIC','CP_VAPID_PRIVATE','CP_VAPID_SUBJECT']:
        os.environ.pop(key,None)
    dotenv=Path(scratch)/'fixture.env'
    dotenv.write_text('CP_VAPID_PUBLIC=original-public\nCP_VAPID_PRIVATE=original-private\nCP_VAPID_SUBJECT=mailto:fixture@example.invalid\n')
    from_file=Settings(_env_file=dotenv)
    dotenv.write_text('CP_VAPID_PUBLIC=changed-public\nCP_VAPID_PRIVATE=changed-private\n')
    dotenv_snapshot={key:getattr(from_file,key) for key in snapshot}
    output = dict(source_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                  command='cd backend && uv run python tests/fixtures/contract/guests/capture_push_config.py',
                  isolation='Settings e _file originais; ambiente temporário, mkdir substituído por observação sem escrita; alteração posterior do ambiente sem reconstruir Settings.',
                  rows=rows, snapshot=snapshot, dotenv_snapshot=dotenv_snapshot)
    Path(__file__).with_name('push-config.json').write_text(json.dumps(output, ensure_ascii=True, indent=2)+'\n')
    print(len(rows))

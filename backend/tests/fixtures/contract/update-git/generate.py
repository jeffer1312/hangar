"""Caracteriza o motor git contra repositórios descartáveis, sem serviço nem rede externa."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--reference', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
reference = args.reference.resolve()
output = args.output.resolve()
commit = subprocess.check_output(['git', '-C', str(reference), 'rev-parse', 'HEAD'], text=True).strip()
sys.path.insert(0, str(reference / 'backend'))
with tempfile.TemporaryDirectory(prefix='hangar-update-reference-') as scratch:
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=str(root/'claude'),
                      GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1',
                      GIT_AUTHOR_NAME='Contract', GIT_AUTHOR_EMAIL='contract@example.invalid',
                      GIT_COMMITTER_NAME='Contract', GIT_COMMITTER_EMAIL='contract@example.invalid',
                      CP_AUTH_TOKEN='contract-owner')
    os.chdir(root)
    from app import atualizar
    from app.config import settings
    atualizar._falta = lambda *programs: []
    atualizar._topologia = lambda: 'manual'
    atualizar._SOU_O_MOTOR = False
    cases = []
    def git(repo, *argv):
        return subprocess.check_output(['git', '-C', str(repo), *argv], text=True, stderr=subprocess.DEVNULL).strip()
    for name in ['clean', 'untracked', 'tracked', 'ahead', 'behind', 'diverged', 'work-branch', 'switch-channel', 'return-from-channel']:
        repo = root / name
        repo.mkdir()
        git(repo, 'init', '-b', 'main')
        (repo/'tracked').write_text('initial\n')
        git(repo, 'add', '.')
        git(repo, 'commit', '-m', 'initial')
        origin = root / (name + '.git')
        git(repo, 'clone', '--bare', str(repo), str(origin))
        git(repo, 'remote', 'add', 'origin', str(origin))
        git(repo, 'fetch', 'origin')
        atualizar.REPO = repo
        state = root / (name + '-state')
        state.mkdir()
        atualizar._base = lambda: state
        settings.update_branch = ''
        if name == 'untracked': (repo/'loose').write_text('untracked')
        if name == 'tracked': (repo/'tracked').write_text('modified')
        if name in ['ahead', 'diverged']:
            (repo/'local').write_text('local')
            git(repo, 'add', '.')
            git(repo, 'commit', '-m', 'local')
        if name in ['behind', 'diverged']:
            clone = root / (name + '-remote')
            git(repo, 'clone', str(origin), str(clone))
            (clone/'remote').write_text('remote')
            git(clone, 'add', '.')
            git(clone, 'commit', '-m', 'remote')
            git(clone, 'push', 'origin', 'main')
            git(repo, 'fetch', 'origin')
        if name == 'work-branch': git(repo, 'checkout', '-b', 'work/task')
        if name == 'switch-channel': settings.update_branch = 'beta'
        if name == 'return-from-channel':
            git(repo, 'checkout', '-b', 'beta')
            (state/'branch').write_text('beta')
        result = atualizar.checar()
        assert result['commit'] == git(repo, 'rev-parse', 'HEAD')
        result['commit'] = '<HEAD>'
        cases.append({'name': name, 'expected': result})
    outputs = []
    for code, stdout, stderr in [(1, 'one\ntwo\n', ''), (0, '', 'diagnostic\n'), (7, 'fatal: authentication failed\n', ''), (1, '', '')]:
        result = subprocess.CompletedProcess(['fixture'], code, stdout, stderr)
        outputs.append({'code': code, 'stdout': stdout, 'stderr': stderr,
                        'tail': atualizar._cauda(result), 'reason': atualizar._motivo_da_falha(result)})
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps({'source_commit': commit, 'command': 'python backend/tests/fixtures/contract/update-git/generate.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/cases.json', 'preflight': cases, 'output': outputs}, ensure_ascii=False, indent=2)+'\n')
    print(f'{len(cases)} pré-voos git reais + {len(outputs)} classificações capturados de {commit}')

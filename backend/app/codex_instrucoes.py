"""Aliases de instruções lidos pelo próprio Codex antes do primeiro turno."""
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
from contextlib import contextmanager

from app.codex_arquivos import gravar, hash_bytes, json_bytes, json_obj

LIMITE_INSTRUCOES = 1024 * 1024


@contextmanager
def _trava(codex_home: Path):
    codex_home.mkdir(parents=True, exist_ok=True)
    with (codex_home / '.hangar-instrucoes.lock').open('a+b') as lock:
        if os.name == 'nt':
            import msvcrt
            if lock.tell() == 0:
                lock.write(b'0')
                lock.flush()
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)
        else:
            import fcntl
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            if os.name == 'nt':
                lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


def limite_instrucoes(codex_home: Path) -> int:
    total = 0
    for path in (codex_home / '.hangar-instrucoes').glob('*.json'):
        fonte = Path(json_obj(path)['fonte'])
        if fonte.is_file():
            total += fonte.stat().st_size + 1024
    config = codex_home / 'config.toml'
    atual = tomllib.loads(config.read_text(encoding='utf-8')).get('project_doc_max_bytes', 32768) if config.exists() else 32768
    if not isinstance(atual, int) or isinstance(atual, bool) or atual < 0:
        raise ValueError('project_doc_max_bytes inválido')
    return max(LIMITE_INSTRUCOES, atual, total)


def _fonte(pasta: Path) -> Path | None:
    for nome in ('CLAUDE.md', 'CLAUDE.MD'):
        path = pasta / nome
        if path.is_file() and path.read_text(encoding='utf-8').strip():
            return path.absolute()
    return None


def _escopos(cwd: Path) -> list[Path]:
    cwd = cwd.resolve()
    caminho = [cwd, *cwd.parents]
    raiz = next((p for p in caminho if (p / '.git').exists()), cwd)
    return list(reversed(caminho[:caminho.index(raiz) + 1]))


def _alias(alvo: Path, fonte: Path | None, registros: Path) -> None:
    registro = registros / (hashlib.sha256(str(alvo).encode()).hexdigest() + '.json')
    anterior = json_obj(registro)
    if fonte is None and not anterior:
        return
    existe = alvo.exists() or alvo.is_symlink()
    raw = alvo.read_bytes() if alvo.exists() else None
    link = os.readlink(alvo) if alvo.is_symlink() else None
    estados = [anterior, anterior.get('anterior', {})]
    proprio = any((alvo.is_symlink() and estado.get('fonte') == str(alvo.resolve())) or (
        not alvo.is_symlink() and estado.get('hash') == hash_bytes(raw) and raw is not None)
        for estado in estados)
    if existe and not proprio and fonte is not None and alvo.is_symlink() and alvo.resolve() == fonte.resolve():
        return  # Um link pessoal já correto não precisa ser adotado nem modificado.
    if existe and not proprio:
        raise ValueError(f'Instruções pessoais preservadas em {alvo}; não é um alias gerenciado.')
    if fonte is None:
        if proprio:
            alvo.unlink()
        registro.unlink(missing_ok=True)
        return
    dados = fonte.read_bytes()
    if proprio and alvo.is_symlink() and alvo.resolve() == fonte.resolve():
        return
    if proprio and not alvo.is_symlink() and raw == dados:
        return
    novo = {'alvo': str(alvo), 'fonte': str(fonte.resolve()), 'hash': hash_bytes(dados)}
    alvo.parent.mkdir(parents=True, exist_ok=True)
    fd, nome = tempfile.mkstemp(prefix='.hangar-instrucoes-', dir=alvo.parent)
    os.close(fd)
    tmp = Path(nome)
    tmp.unlink()
    try:
        try:
            tmp.symlink_to(os.path.relpath(fonte, alvo.parent))
        except OSError:
            tmp.write_bytes(dados)
        # O registro vem antes: uma queda após a troca ainda deixa a autoria verificável.
        transicao = json_bytes({**novo, 'anterior': {k: anterior.get(k) for k in ('fonte', 'hash')}})
        gravar(registro, transicao, registro.read_bytes() if registro.exists() else None)
        if ((alvo.exists() or alvo.is_symlink()) != existe or (alvo.read_bytes() if alvo.exists() else None) != raw
                or (os.readlink(alvo) if alvo.is_symlink() else None) != link):
            raise ValueError(f'Instruções alteradas durante a integração: {alvo}')
        os.replace(tmp, alvo)
        gravar(registro, json_bytes(novo), transicao)
    finally:
        tmp.unlink(missing_ok=True)


def preparar_instrucoes(home: Path, codex_home: Path, cwd: Path | None = None) -> None:
    """Prepara o global e os escopos conhecidos; não sobrescreve overrides pessoais."""
    with _trava(codex_home):
        _preparar(home, codex_home, cwd)


def _preparar(home: Path, codex_home: Path, cwd: Path | None) -> None:
    registros = codex_home / '.hangar-instrucoes'
    _alias(codex_home / 'AGENTS.override.md', _fonte(home / '.claude'), registros)
    projetos = set()
    config = codex_home / 'config.toml'
    if cwd is None and config.exists():
        projetos.update(Path(p) for p in tomllib.loads(config.read_text(encoding='utf-8')).get('projects', {}))
    if cwd is not None:
        projetos.add(cwd)
    # Retoma também aliases de projetos que já passaram pelo lançador.
    for path in registros.glob('*.json') if cwd is None else ():
        alvo = json_obj(path).get('alvo')
        if alvo and Path(alvo).parent != codex_home:
            projetos.add(Path(alvo).parent)
    escopos = {p for projeto in projetos if projeto.is_absolute() and projeto.is_dir()
               for p in _escopos(projeto)}
    for pasta in sorted(escopos):
        fonte = _fonte(pasta)
        _alias(pasta / 'AGENTS.override.md', fonte, registros)
        if fonte is not None:
            _excluir_do_git(pasta)


def _excluir_do_git(pasta: Path) -> None:
    """O alias mora dentro do repositório e é local à instalação: some do `git status` pelo
    `info/exclude`, que não versiona. Fora de repositório (ou sem git) não há o que fazer."""
    try:
        r = subprocess.run(['git', '-C', str(pasta), 'rev-parse', '--git-path', 'info/exclude'],
                           capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return
    if r.returncode != 0 or not r.stdout.strip():
        return
    exclude = Path(r.stdout.strip())
    if not exclude.is_absolute():
        exclude = pasta / exclude
    try:
        linhas = exclude.read_text(encoding='utf-8').splitlines() if exclude.exists() else []
        if 'AGENTS.override.md' in linhas:
            return
        exclude.parent.mkdir(parents=True, exist_ok=True)
        with exclude.open('a', encoding='utf-8') as f:
            f.write(('' if not linhas or linhas[-1] == '' else '\n') + 'AGENTS.override.md\n')
    except OSError:
        return

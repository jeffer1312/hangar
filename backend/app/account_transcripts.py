"""Conversas de uma conta apagada vão para a conta padrão antes de a pasta sumir.

Mesmas regras do `accounts/transcripts.rs` do servidor Rust: o modo de reserva não pode
apagar conversa que o Rust guardaria.
"""
from __future__ import annotations

import filecmp
import os
import shutil
import stat
from pathlib import Path

from app import diag

CLAUDE_FOLDERS = ("projects",)
CODEX_FOLDERS = ("sessions", "archived_sessions")
MERGE_FAILED = "account_transcripts_merge_failed"


class MergeError(Exception):
    """Falha com os caminhos envolvidos: sem eles o erro não diz qual arquivo travou a exclusão."""

    def __init__(self, source: Path | None, target: Path, error: OSError):
        self.source, self.target = source, target
        prefix = f"{source} → " if source is not None else ""
        super().__init__(f"{prefix}{target}: {error}")

    def params(self) -> dict[str, str | None]:
        return {"error": str(self), "source": None if self.source is None else str(self.source),
                "target": str(self.target)}


def keep(home: Path, default: Path, folders: tuple[str, ...], label: str) -> dict[str, int]:
    """Copia as pastas de conversa para a conta padrão, sem sobrescrever nada."""
    count = {"merged": 0, "skipped": 0, "renamed": 0}
    touched: set[Path] = set()
    try:
        for folder in folders:
            source = home / folder
            # Pasta que é link já aponta para outro lugar: nada da conta mora nela.
            if source.is_dir() and not source.is_symlink():
                _tree(source, default / folder, label, count, touched)
        if os.name != "nt":
            for directory in touched:
                try:
                    fd = os.open(directory, os.O_RDONLY)
                    try:
                        os.fsync(fd)
                    finally:
                        os.close(fd)
                except OSError as exc:
                    raise MergeError(None, directory, exc) from exc
    except MergeError as exc:
        # Só o código e o errno: caminho de conversa não entra no diário exportável.
        diag.registrar("conta.apagar.conversas_falharam", "erro", codigo=MERGE_FAILED,
                       **diag.erro_campos(exc))
        raise
    return count


def _tree(source: Path, target: Path, label: str, count: dict[str, int], touched: set[Path]) -> None:
    try:
        entries = list(os.scandir(source))
    except OSError as exc:
        raise MergeError(source, target, exc) from exc
    for entry in entries:
        try:
            is_dir = entry.is_dir(follow_symlinks=False)
            is_file = not is_dir and entry.is_file(follow_symlinks=False)
        except OSError as exc:
            raise MergeError(Path(entry.path), target, exc) from exc
        if is_dir:
            _tree(Path(entry.path), target / entry.name, label, count, touched)
        elif is_file:
            _file(Path(entry.path), target / entry.name, label, count, touched)
        # Link não é seguido: levaria para a conta padrão o que mora fora da conta.


def _file(source: Path, target: Path, label: str, count: dict[str, int], touched: set[Path]) -> None:
    try:
        _make_dirs(target.parent, touched)
    except OSError as exc:
        raise MergeError(source, target.parent, exc) from exc
    attempt = 0
    while True:
        candidate = target if attempt == 0 else _renamed(target, label, attempt)
        try:
            if not os.path.lexists(candidate):
                try:
                    _copy_new(source, candidate)
                except FileExistsError:
                    # Uma sessão da conta padrão criou o mesmo nome agora: reavalia o candidato.
                    continue
                touched.add(candidate.parent)
                count["merged" if attempt == 0 else "renamed"] += 1
                return
            if candidate.is_file() and not candidate.is_symlink() and filecmp.cmp(source, candidate, shallow=False):
                count["skipped"] += 1
                return
        except OSError as exc:
            raise MergeError(source, candidate, exc) from exc
        attempt += 1


def _make_dirs(directory: Path, touched: set[Path]) -> None:
    """Cria só para o dono as pastas que faltam (as do Claude são 0700) e marca o pai de cada
    uma: a entrada da pasta nova também precisa chegar ao disco."""
    if directory.is_dir():
        return
    _make_dirs(directory.parent, touched)
    try:
        directory.mkdir(mode=0o700)
    except FileExistsError:
        if not directory.is_dir():
            raise
        return
    touched.add(directory.parent)


def _renamed(target: Path, label: str, attempt: int) -> Path:
    """`<stem>.from-<label><ext>`, e `-N` a partir da segunda colisão."""
    suffix = "" if attempt == 1 else f"-{attempt}"
    return target.with_name(f"{target.stem}.from-{label}{suffix}{target.suffix}")


def _copy_new(source: Path, target: Path) -> None:
    meta = source.stat()
    times = (meta.st_atime_ns, meta.st_mtime_ns)
    by_fd = os.utime in os.supports_fd
    with open(source, "rb") as src:
        dst = open(target, "xb", opener=_private)
        try:
            with dst:
                shutil.copyfileobj(src, dst)
                dst.flush()
                if dst.tell() != meta.st_size:
                    raise OSError(f"cópia incompleta de {source}")
                if by_fd:
                    os.utime(dst.fileno(), ns=times)
                # Transcript do Claude é 0600: a cópia não pode ficar legível por outros usuários.
                if os.name != "nt":
                    os.fchmod(dst.fileno(), stat.S_IMODE(meta.st_mode))
                os.fsync(dst.fileno())
            if not by_fd:
                # No Windows o utime por caminho não abre arquivo que ainda está aberto.
                os.utime(target, ns=times)
        except BaseException:
            # O arquivo é nosso ("xb"): cópia pela metade não fica para a próxima tentativa.
            target.unlink(missing_ok=True)
            raise


def _private(path: str, flags: int) -> int:
    return os.open(path, flags, 0o600)

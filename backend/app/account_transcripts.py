"""Conversas de uma conta apagada vão para a conta padrão antes de a pasta sumir.

Mesmas regras do `accounts/transcripts.rs` do servidor Rust: o modo de reserva não pode
apagar conversa que o Rust guardaria.
"""
from __future__ import annotations

import filecmp
import os
import shutil
from pathlib import Path

CLAUDE_FOLDERS = ("projects",)
CODEX_FOLDERS = ("sessions", "archived_sessions")
MERGE_FAILED = "account_transcripts_merge_failed"


class MergeError(Exception):
    pass


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
                fd = os.open(directory, os.O_RDONLY)
                try:
                    os.fsync(fd)
                finally:
                    os.close(fd)
    except OSError as exc:
        raise MergeError(str(exc)) from exc
    return count


def _tree(source: Path, target: Path, label: str, count: dict[str, int], touched: set[Path]) -> None:
    with os.scandir(source) as entries:
        for entry in entries:
            if entry.is_dir(follow_symlinks=False):
                _tree(Path(entry.path), target / entry.name, label, count, touched)
            elif entry.is_file(follow_symlinks=False):
                _file(Path(entry.path), target / entry.name, label, count, touched)
            # Link não é seguido: levaria para a conta padrão o que mora fora da conta.


def _file(source: Path, target: Path, label: str, count: dict[str, int], touched: set[Path]) -> None:
    attempt = 0
    while True:
        candidate = target if attempt == 0 else _renamed(target, label, attempt)
        if not os.path.lexists(candidate):
            _copy_new(source, candidate)
            touched.add(candidate.parent)
            count["merged" if attempt == 0 else "renamed"] += 1
            return
        if candidate.is_file() and not candidate.is_symlink() and filecmp.cmp(source, candidate, shallow=False):
            count["skipped"] += 1
            return
        attempt += 1


def _renamed(target: Path, label: str, attempt: int) -> Path:
    """`<stem>.from-<label><ext>`, e `-N` a partir da segunda colisão."""
    suffix = "" if attempt == 1 else f"-{attempt}"
    return target.with_name(f"{target.stem}.from-{label}{suffix}{target.suffix}")


def _copy_new(source: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    meta = source.stat()
    with open(source, "rb") as src, open(target, "xb") as dst:
        try:
            shutil.copyfileobj(src, dst)
            dst.flush()
            if dst.tell() != meta.st_size:
                raise OSError(f"cópia incompleta de {source}")
            os.fsync(dst.fileno())
        except BaseException:
            # O arquivo é nosso ("xb"): cópia pela metade não fica para a próxima tentativa.
            dst.close()
            target.unlink(missing_ok=True)
            raise
    os.utime(target, ns=(meta.st_atime_ns, meta.st_mtime_ns))

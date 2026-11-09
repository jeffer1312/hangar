import json
import os
import threading
from pathlib import Path
from app import atomico

REMOVED_FILE = Path.home() / ".hangar" / "worktrees-removidas.json"
_TAIL = 256 * 1024


_DEEP_TAIL = 8 * 1024 * 1024


def reversed_lines(path: str):
    """Linhas do fim para o começo, em blocos de `_TAIL`, até `_DEEP_TAIL` bytes. Imagem lida pela
    sessão entra no transcript em base64 e enche sozinha os últimos 256 KB."""
    with open(path, "rb") as fh:
        end = pos = fh.seek(0, os.SEEK_END)
        rest: list[bytes] = []   # pedaços da linha ainda cortada; juntar só no `\n` não recopia megas
        while pos > 0 and end - pos < _DEEP_TAIL:
            step = min(_TAIL, pos)
            pos -= step
            fh.seek(pos)
            block = fh.read(step)
            if b"\n" not in block:
                rest.insert(0, block)
                continue
            lines = (block + b"".join(rest)).split(b"\n")
            rest = [lines[0]]
            yield from reversed(lines[1:])
        if pos == 0:
            yield b"".join(rest)


def removed() -> dict[str, str]:
    try:
        data = json.loads(REMOVED_FILE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _entry_bytes(st: os.stat_result) -> int:
    # Blocos ocupados, como o `du`; o Windows não os informa.
    blocks = getattr(st, "st_blocks", None)
    return blocks * 512 if blocks is not None else st.st_size


def _is_link(entry: os.DirEntry) -> bool:
    # Junction do Windows (node_modules do pnpm) não é symlink, mas `is_dir` sem seguir diz que sim.
    return entry.is_symlink() or entry.is_junction()


def _tree_bytes(entry: os.DirEntry, skipped: list[int]) -> int:
    """`skipped[0]` conta o que não deu para ler: o total sai parcial e a tela mostra "≥"."""
    try:
        if _is_link(entry):
            return 0
        if not entry.is_dir(follow_symlinks=False):
            return _entry_bytes(entry.stat(follow_symlinks=False))
        total = 0
        stack = [entry.path]
        while stack:
            try:
                with os.scandir(stack.pop()) as it:
                    for e in it:
                        if _is_link(e):
                            continue
                        if e.is_dir(follow_symlinks=False):
                            stack.append(e.path)
                        else:
                            total += _entry_bytes(e.stat(follow_symlinks=False))
            except OSError:
                skipped[0] += 1
        return total
    except OSError:
        skipped[0] += 1
        return 0


_removed_lock = threading.Lock()


def _write_removed(path: str, main: str | None) -> None:
    """`main=None` tira a entrada (remoção que falhou)."""
    with _removed_lock:   # ler-alterar-gravar: duas remoções juntas perderiam uma entrada
        data = removed()
        if main is None:
            data.pop(path, None)
        else:
            data[path] = main
        REMOVED_FILE.parent.mkdir(parents=True, exist_ok=True)
        tmp = REMOVED_FILE.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, ensure_ascii=False, indent=1), encoding="utf-8")
        atomico.substituir(tmp, REMOVED_FILE)


def record_removed(path: str, main: str) -> None:
    _write_removed(path, main)


def _under(cwd: str | None, path: str) -> bool:
    """`cwd` é `path` ou fica dentro dela, pelo texto ou pelo realpath (home atrás de symlink).
    Pasta sumida: o realpath resolve só a parte que ainda existe, o que basta para o symlink."""
    if not cwd:
        return False
    for c, p in ((cwd, path), (os.path.realpath(cwd), os.path.realpath(path))):
        if c == p or c.startswith(p.rstrip("/") + "/"):
            return True
    return False


def redirect(cwd: str | None) -> str | None:
    """Retomar uma conversa cuja worktree foi apagada abre na pasta principal."""
    if not cwd or os.path.isdir(cwd):
        return cwd
    for path, main in removed().items():
        if _under(cwd, path):
            return main
    return cwd

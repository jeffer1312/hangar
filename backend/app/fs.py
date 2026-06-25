import os
from pathlib import Path

from app.config import resolve_scan_roots, settings
from app.models import FsEntry, FsRoot, FsScanResult


class FsError(Exception):
    """Falha de fronteira/acesso do fs-scanner. Carrega o status HTTP que a API deve
    devolver: 403 raiz nao liberada, 400 escape/nao-diretorio, 404 caminho ausente."""

    def __init__(self, status: int, detail: str):
        self.status = status
        self.detail = detail
        super().__init__(detail)


def _real(p: str) -> Path:
    # expanduser + realpath: resolve ~, '..' e symlinks ANTES de qualquer checagem,
    # pra que a comparacao de fronteira seja sobre o caminho real (nao o lexico).
    return Path(os.path.realpath(os.path.expanduser(p)))


def _within(child: Path, root: Path) -> bool:
    # Containment robusto (ambos ja realpath): igualdade ou descendencia real.
    return child == root or child.is_relative_to(root)


def list_roots() -> list[FsRoot]:
    # So as raizes da allowlist viram chips. name = basename do caminho real.
    return [FsRoot(name=r.name, path=str(r)) for r in resolve_scan_roots(settings)]


def scan_dir(root: str, path: str | None = None) -> FsScanResult:
    """Lista os subdiretorios imediatos de `path` (default = `root`).

    Seguranca (espelha o espirito de registry.sanitize_cwd):
      1. `root` precisa ser EXATAMENTE uma das raizes configuradas (comparado por realpath);
      2. `path` e realpath-resolvido e precisa ficar CONTIDO na raiz -> '..'/symlink que
         escapa sao rejeitados;
      3. nunca lista fora de uma raiz liberada.
    """
    roots = resolve_scan_roots(settings)

    # 1) raiz precisa casar exatamente uma da allowlist (por realpath).
    root_real = _real(root)
    allowed = next((r for r in roots if r == root_real), None)
    if allowed is None:
        raise FsError(403, "root not allowed")

    # 2) path default = a propria raiz; senao realpath + exige conter dentro da raiz.
    target = allowed if not path else _real(path)
    if not _within(target, allowed):
        raise FsError(400, "path escapes its root")

    # 3) existencia / tipo.
    if not target.exists():
        raise FsError(404, "path not found")
    if not target.is_dir():
        raise FsError(400, "not a directory")

    # 4) varre os filhos imediatos. Pasta valida porem ilegivel -> vazio + erro claro
    #    (nao vaza nada e nao derruba a UI).
    try:
        raw = list(os.scandir(target))
    except PermissionError:
        return FsScanResult(entries=[], error="permission_denied")
    except OSError:
        return FsScanResult(entries=[], error="unreadable")

    entries: list[FsEntry] = []
    for e in raw:
        if e.name.startswith("."):
            continue  # esconde dot-dirs (.git, .venv...): ruido pra um seletor de projeto
        try:
            if not e.is_dir():  # segue symlink; so diretorios viram linha
                continue
            # symlink que aponta pra fora da raiz nunca aparece (nao lista fora do allowlist)
            if e.is_symlink() and not _within(_real(e.path), allowed):
                continue
            child = Path(e.path)
            entries.append(
                FsEntry(
                    name=e.name,
                    path=str(child),
                    is_git=(child / ".git").exists(),
                    has_claude_md=(child / "CLAUDE.md").is_file(),
                    mtime=e.stat().st_mtime,
                )
            )
        except OSError:
            continue  # entrada sumiu/ilegivel no meio do scan -> ignora

    entries.sort(key=lambda x: x.mtime or 0.0, reverse=True)
    return FsScanResult(entries=entries, error=None)

"""Escritas da integração: backup restrito, comparação antes da troca e lock por instalação."""
from __future__ import annotations

import asyncio
import hashlib
import json
import os
import tempfile
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Callable

from app import atomico


class AlteradoExternamente(RuntimeError):
    pass


def ler(path: Path) -> bytes | None:
    try:
        return path.read_bytes()
    except FileNotFoundError:
        if path.is_symlink():
            raise ValueError(f"Link sem destino: {path}")
        return None


def json_obj(path: Path) -> dict:
    raw = ler(path)
    if raw is None:
        return {}
    data = json.loads(raw)
    if not isinstance(data, dict):
        raise ValueError(f"Objeto JSON esperado em {path}")
    return data


def json_bytes(data: dict) -> bytes:
    return (json.dumps(data, ensure_ascii=False, indent=2) + "\n").encode()


def hash_bytes(raw: bytes | None) -> str:
    return hashlib.sha256(raw or b"").hexdigest()


def gravar(path: Path, raw: bytes, esperado: bytes | None, backups: Path | None = None) -> bool:
    """Não segue links ao substituir: persona ligada ao Claude não pode reescrever a fonte."""
    if esperado == raw:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    modo = (path.stat().st_mode & 0o777) if path.exists() else 0o600
    fd, nome = tempfile.mkstemp(prefix=f".{path.name}.hangar-", dir=path.parent)
    tmp = Path(nome)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(raw)
            f.flush()
            os.fsync(f.fileno())
        os.chmod(tmp, modo)
        if ler(path) != esperado:
            raise AlteradoExternamente(f"Arquivo alterado durante a integração: {path.name}")
        if backups is not None and esperado is not None:
            backup(path, esperado, backups)
        # A comparação fica junto da troca; o reconciliador repete uma etapa invalidada.
        if ler(path) != esperado:
            raise AlteradoExternamente(f"Arquivo alterado durante a integração: {path.name}")
        atomico.substituir(tmp, path)
        return True
    finally:
        tmp.unlink(missing_ok=True)


def backup(path: Path, raw: bytes, raiz: Path) -> None:
    raiz.mkdir(parents=True, exist_ok=True, mode=0o700)
    nome = hashlib.sha256(str(path.absolute()).encode()).hexdigest() + ".json"
    dst = raiz / nome
    data = {"path": str(path), "conteudo_hex": raw.hex(),
            "symlink": os.readlink(path) if path.is_symlink() else None}
    try:
        fd = os.open(dst, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        return
    with os.fdopen(fd, "wb") as f:
        f.write(json_bytes(data))


def transformar(path: Path, fn: Callable[[bytes | None], bytes], backups: Path) -> bool:
    for _ in range(3):
        raw = ler(path)
        try:
            return gravar(path, fn(raw), raw, backups)
        except AlteradoExternamente:
            continue
    raise AlteradoExternamente(f"Arquivo continua mudando: {path.name}; tentar novamente")


@asynccontextmanager
async def exclusivo(path: Path):
    """Lock do sistema operacional, liberado inclusive se o processo morrer."""
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
    f = os.fdopen(fd, "r+b")
    adquirido = False
    try:
        if os.fstat(f.fileno()).st_size == 0:
            f.write(b"0")
            f.flush()
        while not adquirido:
            try:
                if os.name == "nt":
                    import msvcrt
                    f.seek(0)
                    msvcrt.locking(f.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                adquirido = True
            except (BlockingIOError, PermissionError):
                await asyncio.sleep(0.1)
            except OSError as exc:
                if os.name != "nt" or exc.errno not in (13, 36):
                    raise
                await asyncio.sleep(0.1)
        yield
    finally:
        if adquirido:
            if os.name == "nt":
                import msvcrt
                f.seek(0)
                msvcrt.locking(f.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(f.fileno(), fcntl.LOCK_UN)
        f.close()


def remapear(valor, antigo: Path, novo: Path):
    if isinstance(valor, str):
        return valor.replace(str(antigo), str(novo)).replace(antigo.as_posix(), novo.as_posix())
    if isinstance(valor, list):
        return [remapear(v, antigo, novo) for v in valor]
    if isinstance(valor, dict):
        return {k: remapear(v, antigo, novo) for k, v in valor.items()}
    return valor


def mesclar_hooks(atual: dict, fonte: dict, anteriores: dict) -> dict:
    """Identifica comandos por evento e matcher; valida tudo antes de calcular remoções."""
    import copy

    def validar(documento: dict, rotulo: str) -> dict:
        if not isinstance(documento, dict) or not isinstance(documento.get("hooks", {}), dict):
            raise ValueError(f"hooks.json inválido ({rotulo}): objeto hooks esperado")
        try:
            json.dumps(documento, allow_nan=False)
        except (TypeError, ValueError) as exc:
            raise ValueError(f"hooks.json inválido ({rotulo}): conteúdo não é JSON válido") from exc
        eventos = documento.get("hooks", {})
        for evento, grupos in eventos.items():
            if not isinstance(evento, str) or not isinstance(grupos, list):
                raise ValueError(f"hooks.json inválido ({rotulo}): lista de grupos esperada")
            for grupo in grupos:
                if not isinstance(grupo, dict) or not isinstance(grupo.get("hooks"), list):
                    raise ValueError(f"hooks.json inválido ({rotulo}): grupo de hooks esperado")
                if "matcher" in grupo and not isinstance(grupo["matcher"], str):
                    raise ValueError(f"hooks.json inválido ({rotulo}): matcher precisa ser texto")
                for hook in grupo["hooks"]:
                    if not isinstance(hook, dict) or not isinstance(hook.get("type"), str) or not hook["type"]:
                        raise ValueError(f"hooks.json inválido ({rotulo}): tipo de hook esperado")
                    if hook["type"] == "command" and (
                        not isinstance(hook.get("command"), str) or not hook["command"].strip()
                    ):
                        raise ValueError(f"hooks.json inválido ({rotulo}): comando precisa ser texto")
                    if "timeout" in hook and (
                        isinstance(hook["timeout"], bool) or not isinstance(hook["timeout"], (int, float))
                        or hook["timeout"] < 0
                    ):
                        raise ValueError(f"hooks.json inválido ({rotulo}): timeout precisa ser número não negativo")
        return eventos

    def identidade(grupo: dict, hook: dict) -> tuple:
        matcher = grupo.get("matcher", "")
        if hook["type"] == "command":
            return matcher, "command", hook["command"]
        # Hooks de outros tipos só são substituídos quando o payload é reconhecido;
        # ter matcher igual não autoriza retirar prompts particulares.
        return matcher, hook["type"], json.dumps(hook, sort_keys=True, ensure_ascii=True)

    validar(atual, "atual")
    desejados_por_evento = validar(fonte, "fonte")
    antigos_por_evento = validar(anteriores, "manifesto anterior")
    out = copy.deepcopy(atual)
    hooks = out.setdefault("hooks", {})
    for evento in sorted(set(desejados_por_evento) | set(antigos_por_evento)):
        desejados = desejados_por_evento.get(evento, [])
        conhecidos = [*desejados, *antigos_por_evento.get(evento, [])]
        substituidos = {
            identidade(grupo, hook) for grupo in conhecidos for hook in grupo["hooks"]
        }
        grupos = []
        for grupo in hooks.get(evento, []):
            resto = [hook for hook in grupo["hooks"] if identidade(grupo, hook) not in substituidos]
            if resto or (not grupo["hooks"] and grupo not in conhecidos):
                grupos.append({**grupo, "hooks": resto})
        grupos.extend(copy.deepcopy(desejados))
        if grupos:
            hooks[evento] = grupos
        else:
            hooks.pop(evento, None)
    return out

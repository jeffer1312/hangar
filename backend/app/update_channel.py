"""Canal do Atualizar escolhido pelo dono, persistido sem reiniciar o servidor."""
import io
import logging
import os
import re
import subprocess
import tempfile
import threading
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel, StrictStr
from dotenv.parser import parse_stream

from app import atomico, atualizar, diag, git_ops, guest_users, peers, user_data
from app.auth import require_auth
from app.config import settings, valid_update_branch
from app.mensagens import erro
from app.guest_header import guest_of

ENV_FILE = user_data.path(".env")
_lock = threading.Lock()
_log = logging.getLogger("hangar.update_channel")


def _record_failure(code: str, *, exception: Exception | None = None, result: subprocess.CompletedProcess | None = None, branch: str = "") -> None:
    fields = diag.erro_campos(exception) if exception is not None else {}
    detail = branch
    if result is not None:
        fields["retorno"] = result.returncode
        tail = git_ops._scrub("\n".join((result.stderr or "").splitlines()[-3:]))
        # URLs do Git podem carregar credenciais também na query e no fragmento.
        tail = re.sub(r"https?://[^\s'\"<>]+", lambda match: re.split(r"[?#]", match[0])[0], tail)
        detail = tail.strip()[:300]
    diag.registrar("update_channel.failed", "erro", codigo=code, detalhe=detail, **fields)
    _log.error("update_channel.failed codigo=%s causa=%s detalhe=%s", code, fields, detail)


def _git_request(code: str, *args: str, timeout: float = 20) -> subprocess.CompletedProcess:
    try:
        return git_ops._run(str(atualizar.REPO), *args, timeout=timeout)
    except git_ops.GitError as exc:
        _record_failure(code, exception=exc)
        message = "Não foi possível ler a branch instalada." if code == "checkout" else "Não foi possível conferir a branch no origin. Tente novamente."
        raise HTTPException(503, detail=erro(f"update_channel_{code}", message)) from exc


def require_owner(request: Request) -> None:
    if guest_of(request) is not None or guest_users.current.get() is not None:
        raise HTTPException(403, detail=erro("update_channel_owner", "Só o dono pode mudar o canal de testes."))


router = APIRouter(prefix="/api/update-channel", dependencies=[Depends(require_auth), Depends(require_owner)])


class ChannelRequest(BaseModel):
    branch: StrictStr


def _checkout_branch() -> str:
    result = _git_request("checkout", "rev-parse", "--abbrev-ref", "HEAD")
    if result.returncode or "\ufffd" in result.stdout:
        _record_failure("checkout", result=result)
        raise HTTPException(503, detail=erro("update_channel_checkout", "Não foi possível ler a branch instalada."))
    return result.stdout.strip()


def _snapshot(checkout: str) -> dict:
    return {"branch": settings.update_branch, "checkout_branch": checkout,
            "last_branch": settings.update_last_branch or settings.update_branch}


@router.get("")
def read_channel() -> dict:
    try:
        with _lock:
            return _snapshot(_checkout_branch())
    except HTTPException:
        raise
    except Exception as exc:
        _record_failure("checkout", exception=exc)
        raise HTTPException(503, detail=erro("update_channel_checkout", "Não foi possível ler a branch instalada.")) from exc


def _write_env(branch: str, last: str) -> None:
    ENV_FILE.parent.mkdir(parents=True, exist_ok=True)
    with open(ENV_FILE.with_name(ENV_FILE.name + ".lock"), "a+", encoding="utf-8") as lock:
        peers._travar(lock)
        try:
            try:
                raw = ENV_FILE.read_bytes().decode("utf-8-sig")
            except FileNotFoundError:
                raw = ""
            newline = "\r\n" if "\r\n" in raw else "\n"
            values = {"CP_UPDATE_BRANCH": branch, "CP_UPDATE_LAST_BRANCH": last}
            output = []
            seen = set()
            # O parser reconhece valores multilinha sem confundir seu conteúdo com outra chave.
            for binding in parse_stream(io.StringIO(raw)):
                line = binding.original.string
                if binding.key in values:
                    key = binding.key
                    if key not in seen:
                        assignment = re.match(r"^(\s*(?:export[^\S\r\n]+)?(?:'[^']+'|[^=\#\s]+)[^\S\r\n]*=[^\S\r\n]*)", line)
                        if assignment:
                            prefix = assignment[0]
                            rest = line[len(prefix):]
                            quoted = re.match(r"'(?:\\'|[^'])*'|\"(?:\\\"|[^\"])*\"", rest)
                            if quoted:
                                suffix = rest[quoted.end():]
                            else:
                                value_line = rest.splitlines()[0] if rest and rest[0] not in "\r\n" else ""
                                comment = re.search(r"[^\S\r\n]+#", value_line)
                                end = comment.start() if comment else len(value_line.rstrip())
                                suffix = rest[end:]
                            output.append(prefix + values[key] + suffix)
                        else:
                            ending = "\r\n" if line.endswith("\r\n") else "\n" if line.endswith("\n") else ""
                            output.append(f"{key}={values[key]}{ending}")
                        seen.add(key)
                else:
                    output.append(line)
            for key, value in values.items():
                if key not in seen:
                    if output and not output[-1].endswith("\n"):
                        output[-1] += newline
                    output.append(f"{key}={value}{newline}")
            # Órfãos contêm segredos do .env; a trava impede apagar o temporário de outro escritor.
            for stale in ENV_FILE.parent.glob(ENV_FILE.name + ".*.tmp"):
                stale.unlink(missing_ok=True)
            fd, name = tempfile.mkstemp(dir=ENV_FILE.parent, prefix=ENV_FILE.name + ".", suffix=".tmp")
            tmp = Path(name)
            try:
                with os.fdopen(fd, "wb") as stream:
                    stream.write("".join(output).encode("utf-8"))
                    stream.flush()
                    os.fsync(stream.fileno())
                os.chmod(tmp, 0o600)
                atomico.substituir(tmp, ENV_FILE)
            finally:
                tmp.unlink(missing_ok=True)
        finally:
            peers._destravar(lock)


@router.put("")
def write_channel(body: ChannelRequest) -> dict:
    branch = body.branch.strip()
    if not valid_update_branch(branch):
        raise HTTPException(400, detail=erro("update_channel_invalid", "Nome de branch inválido."))
    target = branch or "main"
    try:
        with _lock:
            if atualizar.estado_para_tela().get("fase") == "rodando":
                raise HTTPException(409, detail=erro("update_channel_busy", "Aguarde a atualização terminar antes de mudar o canal."))
            checkout = _checkout_branch()
            if branch:
                ref = f"refs/heads/{target}"
                result = _git_request("origin", "ls-remote", "--exit-code", "--heads", "origin", ref)
                if result.returncode == 2:
                    raise HTTPException(400, detail=erro("update_channel_missing", "A branch não existe no origin.", branch=target))
                if result.returncode or "\ufffd" in result.stdout:
                    _record_failure("origin", result=result)
                    raise HTTPException(503, detail=erro("update_channel_origin", "Não foi possível conferir a branch no origin. Tente novamente."))
                if not any(line.split("\t")[-1] == ref for line in result.stdout.splitlines()):
                    raise HTTPException(400, detail=erro("update_channel_missing", "A branch não existe no origin.", branch=target))
            # Usa a mesma vez do Atualizar para ele não nascer durante a troca do canal.
            if not atualizar._tomar_a_vez():
                raise HTTPException(409, detail=erro("update_channel_busy", "Aguarde a atualização terminar antes de mudar o canal."))
            try:
                last = branch or settings.update_last_branch or settings.update_branch
                _write_env(branch, last)
                settings.update_branch, settings.update_last_branch = branch, last
                # Só substitui overrides herdados; sem eles, o motor lê o .env.
                for key, value in (("CP_UPDATE_BRANCH", branch), ("CP_UPDATE_LAST_BRANCH", last)):
                    if key in os.environ:
                        os.environ[key] = value
            finally:
                atualizar._soltar_a_vez()
            diag.registrar("update_channel.changed", codigo="applied", detalhe=target)
            return _snapshot(checkout)
    except HTTPException:
        raise
    except Exception as exc:
        _record_failure("write", exception=exc, branch=target)
        raise HTTPException(500, detail=erro("update_channel_write", "Não foi possível gravar o canal. O canal anterior continua valendo.", motivo=type(exc).__name__)) from exc

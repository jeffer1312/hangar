import os
import re
import secrets
import time
from pathlib import Path

# Qualquer tipo de arquivo (imagem, video, pdf, ...). A extensao vem do filename do cliente,
# sanitizada; o NOME e gerado pelo servidor (sem path traversal). O assistente le/preview pelo path.
MAX_BYTES = 100 * 1024 * 1024  # 100 MiB
UPLOAD_SUBDIR = ".claude-pocket-uploads"
_EXT_RE = re.compile(r"[^a-z0-9]")


class UploadError(Exception):
    """Erro de upload com status HTTP pra o endpoint mapear direto."""
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def _safe_ext(filename: str | None) -> str:
    """Extensao do filename do cliente, sanitizada -> [a-z0-9] ate 8 chars. So a EXTENSAO
    vem do cliente; o nome do arquivo e gerado pelo servidor. Fallback 'bin' sem extensao."""
    ext = Path(filename or "").suffix.lower().lstrip(".")
    ext = _EXT_RE.sub("", ext)[:8]
    return ext or "bin"


def save_upload(cwd: str, content: bytes, filename: str | None) -> str:
    """Salva os bytes em <cwd>/.claude-pocket-uploads/ com nome gerado pelo servidor
    (nunca o filename do cliente -> sem path traversal). Devolve o path absoluto.
    Levanta UploadError(status, detail) em arquivo vazio / grande demais."""
    if not content:
        raise UploadError(400, "arquivo vazio")
    if len(content) > MAX_BYTES:
        raise UploadError(413, "arquivo maior que 100 MiB")

    ext = _safe_ext(filename)
    base = Path(os.path.realpath(cwd)) / UPLOAD_SUBDIR
    base.mkdir(parents=True, exist_ok=True)
    fname = f"{int(time.time())}-{secrets.token_hex(3)}.{ext}"
    dest = base / fname
    real_base = os.path.realpath(base)
    real_dest = os.path.realpath(dest)
    if not (real_dest == os.path.join(real_base, fname)):
        raise UploadError(400, "caminho invalido")
    Path(real_dest).write_bytes(content)
    return real_dest


def resolve_upload(cwd: str, filename: str) -> str:
    """Resolve <cwd>/.claude-pocket-uploads/<filename> com seguranca, pra servir o arquivo.
    Rejeita filename com separador/.. (400) e arquivo inexistente (404)."""
    if "/" in filename or "\\" in filename or ".." in filename or not filename:
        raise UploadError(400, "filename invalido")
    base = Path(os.path.realpath(cwd)) / UPLOAD_SUBDIR
    real_base = os.path.realpath(base)
    real = os.path.realpath(base / filename)
    if real != os.path.join(real_base, filename):
        raise UploadError(400, "caminho invalido")
    if not os.path.isfile(real):
        raise UploadError(404, "arquivo nao encontrado")
    return real

import os
import secrets
import time
from pathlib import Path

# content-type -> extensao. So imagens (o assistente le via Read).
ALLOWED: dict[str, str] = {
    "image/png": "png",
    "image/jpeg": "jpg",
    "image/webp": "webp",
    "image/gif": "gif",
}
MAX_BYTES = 10 * 1024 * 1024  # 10 MiB
UPLOAD_SUBDIR = ".claude-pocket-uploads"


class UploadError(Exception):
    """Erro de upload com status HTTP pra o endpoint mapear direto."""
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def save_upload(cwd: str, content: bytes, content_type: str | None) -> str:
    """Salva os bytes da imagem em <cwd>/.claude-pocket-uploads/ com nome gerado pelo
    servidor (nunca o filename do cliente -> sem path traversal). Devolve o path absoluto.
    Levanta UploadError(status, detail) em tipo invalido / vazio / grande demais."""
    ext = ALLOWED.get((content_type or "").split(";")[0].strip().lower())
    if ext is None:
        raise UploadError(415, "tipo de imagem nao suportado")
    if not content:
        raise UploadError(400, "arquivo vazio")
    if len(content) > MAX_BYTES:
        raise UploadError(413, "imagem maior que 10 MiB")

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

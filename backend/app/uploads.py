import hashlib
import os
import re
import secrets
import time
import unicodedata
from pathlib import Path

# Qualquer tipo de arquivo (imagem, video, pdf, ...). A extensao vem do filename do cliente,
# sanitizada; o NOME e gerado pelo servidor (sem path traversal). O assistente le/preview pelo path.
MAX_BYTES = 100 * 1024 * 1024  # 100 MiB
_EXT_RE = re.compile(r"[^a-z0-9]")
_SLUG_RE = re.compile(r"[^A-Za-z0-9._-]")


def _raiz() -> Path:
    """A raiz do cofre de anexos. Seam de teste: a suíte troca isto para não escrever no HOME real."""
    return Path.home() / ".hangar" / "uploads"


def _slug(txt: str) -> str:
    """Nome de pasta seguro. Acento vira a letra base antes do filtro, senão "Área de trabalho"
    sairia como "rea-de-trabalho". `..` vira `_`: o nome vem de fora e é concatenado no caminho."""
    plano = unicodedata.normalize("NFKD", txt).encode("ascii", "ignore").decode()
    s = _SLUG_RE.sub("-", plano).strip("-") or "_"
    return "_" if s in (".", "..") else s[:64]


def _projeto(cwd: str) -> str:
    """Nome da pasta do projeto + 6 hex do caminho inteiro. O nome sozinho colide: dois checkouts
    de mesmo basename cairiam no mesmo balde, e o `prune_old` apagaria anexo de um varrendo o outro."""
    real = os.path.realpath(cwd)
    digest = hashlib.sha256(real.encode("utf-8", "surrogateescape")).hexdigest()[:6]
    return f"{_slug(Path(real).name)}-{digest}"


def _base(cwd: str, sessao: str) -> Path:
    """`~/.hangar/uploads/<projeto>/<id da sessão>/`. Fora do cwd porque o `.gitignore` que esconde
    a pasta é o DESTE repo — em qualquer outro ela aparece untracked. Sem migração do que já existe.

    `sessao` é o **id** (`models.session_key`), não o nome: nome muda com o rename e levaria a
    galeria junto. Quem chama cai no nome só enquanto o transcript não nasceu."""
    return _raiz() / _projeto(cwd) / _slug(sessao)


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


def save_upload(cwd: str, sessao: str, content: bytes, filename: str | None) -> str:
    """Salva os bytes em ~/.hangar/uploads/<projeto>/<sessão>/ com nome gerado pelo servidor
    (nunca o filename do cliente -> sem path traversal). Devolve o path absoluto.
    Levanta UploadError(status, detail) em arquivo vazio / grande demais."""
    if not content:
        raise UploadError(400, "arquivo vazio")
    if len(content) > MAX_BYTES:
        raise UploadError(413, "arquivo maior que 100 MiB")

    ext = _safe_ext(filename)
    base = _base(cwd, sessao)
    base.mkdir(parents=True, exist_ok=True)
    fname = f"{int(time.time())}-{secrets.token_hex(3)}.{ext}"
    dest = base / fname
    real_base = os.path.realpath(base)
    real_dest = os.path.realpath(dest)
    if not (real_dest == os.path.join(real_base, fname)):
        raise UploadError(400, "caminho invalido")
    Path(real_dest).write_bytes(content)
    return real_dest


def prune_old(cwd: str, days: int) -> int:
    """Apaga anexos com mais de `days` dias e devolve quantos saíram. days <= 0 = não limpa.

    Varre o projeto inteiro, não só a sessão que está enviando: sessão encerrada também cresce.
    Roda no upload em vez de num job — sem agendador pra manter. Erro de arquivo individual não
    derruba a varredura: a limpeza é higiene, não pode custar o anexo do usuário.
    """
    if days <= 0:
        return 0
    projeto = _base(cwd, "_").parent
    if not projeto.is_dir():
        return 0
    corte = time.time() - days * 86400
    n = 0
    for f in projeto.rglob("*"):
        try:
            if f.is_file() and f.stat().st_mtime < corte:
                f.unlink()
                n += 1
        except OSError:
            continue
    return n


def list_uploads(cwd: str, sessao: str, retention_days: int) -> list[dict]:
    """Anexos da sessão (mais recente primeiro) pra galeria: filename, size, mtime, expires_in_days.

    A expiração vem daqui e não da UI porque quem sabe o prazo é o servidor (`upload_retention_days`);
    o front só desenha o número. retention_days <= 0 = não expira -> None.

    O valor PODE ser <= 0: o prune só roda no upload, então um anexo vencido continua listado até
    alguém enviar o próximo. Mentir "0.1 dia" esconderia justamente o arquivo prestes a sumir.
    Erro de arquivo individual pula o item — a galeria inteira não pode cair por um stat quebrado.
    """
    base = _base(cwd, sessao)
    if not base.is_dir():
        return []
    agora = time.time()
    out: list[dict] = []
    for f in base.iterdir():
        try:
            st = f.stat()
            if not f.is_file():
                continue
        except OSError:
            continue
        out.append({
            "filename": f.name,
            "size": st.st_size,
            "mtime": st.st_mtime,
            "expires_in_days": (
                None if retention_days <= 0
                else retention_days - (agora - st.st_mtime) / 86400
            ),
        })
    # Mesmo segundo: o nome desempata, senão a ordem dependeria do sistema de arquivos.
    out.sort(key=lambda d: d["filename"])
    out.sort(key=lambda d: d["mtime"], reverse=True)
    return out


def resolve_upload(cwd: str, sessao: str, filename: str) -> str:
    """Resolve ~/.hangar/uploads/<projeto>/<sessão>/<filename> com seguranca, pra servir o arquivo.
    Rejeita filename com separador/.. (400) e arquivo inexistente (404)."""
    if "/" in filename or "\\" in filename or ".." in filename or not filename:
        raise UploadError(400, "filename invalido")
    base = _base(cwd, sessao)
    real_base = os.path.realpath(base)
    real = os.path.realpath(base / filename)
    if real != os.path.join(real_base, filename):
        raise UploadError(400, "caminho invalido")
    if not os.path.isfile(real):
        raise UploadError(404, "arquivo nao encontrado")
    return real


def _inside(path: str, base: str) -> bool:
    path, base = os.path.normcase(path), os.path.normcase(base)
    return path.startswith(base.rstrip(os.sep) + os.sep)


def resolve_session_audio(cwd: str, sessao: str, ref: str, *, allow_absolute: bool) -> str:
    """Áudio a transcrever. Nome solto = pasta da sessão atual (`resolve_upload`). Caminho absoluto
    = um `path` devolvido antes, aceito em qualquer pasta de sessão DESTE projeto: a pasta é chaveada
    pelo transcript, e depois de um `/clear` o áudio ficou na pasta do anterior."""
    # NUL faz as funções de caminho levantarem ValueError, que viraria 500 em vez de 400.
    if "\x00" in ref:
        raise UploadError(400, "caminho invalido")
    # Caminho de rede antes de tudo: no Windows, resolver `\\host\share` já é uma conexão SMB.
    if ref.startswith(("\\\\", "//")):
        raise UploadError(400, "caminho de rede recusado")
    if not os.path.isabs(ref):
        return resolve_upload(cwd, sessao, ref)
    if not allow_absolute:
        raise UploadError(403, "caminho absoluto so para o dono")
    if ".." in Path(ref).parts:
        raise UploadError(400, "caminho invalido")
    base = _raiz() / _projeto(cwd)
    # Primeiro sem tocar no disco pelo caminho de fora; a pasta do projeto é nossa, pode ser
    # resolvida (HOME atrás de um link devolve o caminho real no /upload).
    if not (_inside(os.path.abspath(ref), os.path.abspath(base))
            or _inside(os.path.abspath(ref), os.path.realpath(base))):
        raise UploadError(400, "caminho invalido")
    real = os.path.realpath(ref)
    # Exatamente <projeto>/<pasta da sessão>/<arquivo>, já com links resolvidos.
    if os.path.normcase(os.path.dirname(os.path.dirname(real))) != os.path.normcase(os.path.realpath(base)):
        raise UploadError(400, "caminho invalido")
    if not os.path.exists(real):
        raise UploadError(404, "arquivo nao encontrado")
    if not os.path.isfile(real):
        raise UploadError(400, "caminho invalido")
    return real

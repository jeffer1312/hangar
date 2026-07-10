import http.client
import secrets
import urllib.error
import urllib.request

from app.config import settings
from app.uploads import _safe_ext

# Transcricao de audio via Groq (whisper-large-v3-turbo). Groq aceita webm/mp4/m4a/mp3/wav/ogg
# direto -> sem pre-conversao com ffmpeg. HTTP feito com urllib (stdlib): multipart montado a mao,
# zero dep nova. A chave vem de settings.groq_api_key (CP_GROQ_API_KEY no .env ou GROQ_API_KEY no env).
GROQ_URL = "https://api.groq.com/openai/v1/audio/transcriptions"
GROQ_MODEL = "whisper-large-v3-turbo"


class TranscribeError(Exception):
    """Erro de transcricao com status HTTP pra o endpoint mapear direto."""
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def build_multipart(filename: str, content: bytes) -> tuple[bytes, str]:
    """Monta um corpo multipart/form-data (model + response_format + file) e devolve (body, boundary).
    Separado da chamada de rede pra ser testavel sem tocar na Groq."""
    boundary = "----claudepocket" + secrets.token_hex(16)
    b = boundary.encode()
    parts: list[bytes] = []
    for name, value in (("model", GROQ_MODEL), ("response_format", "text")):
        parts += [b"--" + b,
                  f'Content-Disposition: form-data; name="{name}"'.encode(),
                  b"", value.encode()]
    parts += [b"--" + b,
              f'Content-Disposition: form-data; name="file"; filename="{filename}"'.encode(),
              b"Content-Type: application/octet-stream", b"", content]
    parts += [b"--" + b + b"--", b""]
    return b"\r\n".join(parts), boundary


def transcribe(content: bytes, filename: str | None) -> str:
    """Transcreve os bytes de audio via Groq e devolve o texto em UMA linha (send-keys rejeita '\\n').
    Levanta TranscribeError(status, detail): 503 sem chave, 502 falha/erro da Groq."""
    api_key = settings.groq_api_key.strip()
    if not api_key:
        raise TranscribeError(503, "GROQ_API_KEY (ou CP_GROQ_API_KEY) nao configurada no backend")
    # Nome enviado a Groq: FIXO no servidor, so a extensao sanitizada (_safe_ext) — nunca o nome cru do
    # cliente, que interpolado no header Content-Disposition permitiria injecao de aspas/CRLF (partes/campos
    # extras no multipart). A Groq so usa a extensao pra detectar o formato. 'bin' (sem ext) -> webm.
    ext = _safe_ext(filename)
    if ext == "bin":
        ext = "webm"
    body, boundary = build_multipart(f"audio.{ext}", content)
    req = urllib.request.Request(
        GROQ_URL, data=body, method="POST",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            # O Cloudflare da Groq bane o UA padrao do urllib ("Python-urllib/..") com 403 code 1010.
            # Um UA normal passa.
            "User-Agent": "claude-pocket/1.0",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            text = resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        try:
            detail = e.read().decode("utf-8", "replace")[:300]
        except (OSError, http.client.HTTPException):
            # ler o corpo de erro tambem e um read() de socket -> pode cair/timeout. Nao deixa
            # vazar cru (viraria 500); mantem o 502 com o codigo, sem o corpo.
            detail = "(sem corpo)"
        raise TranscribeError(502, f"Groq {e.code}: {detail}")
    except (OSError, http.client.HTTPException) as e:
        # OSError cobre URLError (conexao) e TimeoutError/socket.timeout no read(); http.client cobre
        # IncompleteRead (conexao cai no meio da resposta). Sem isto, timeout no read vazaria como 500.
        raise TranscribeError(502, f"falha ao contatar a Groq: {e}")
    # response_format=text -> corpo e o texto puro. Achata espacos/quebras numa linha so.
    return " ".join(text.split())

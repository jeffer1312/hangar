import http.client
import secrets
import urllib.error
import urllib.request

from app.config import settings
from app import runtime_config
from app.uploads import _safe_ext

# Transcricao de audio via Groq (whisper-large-v3-turbo). Groq aceita webm/mp4/m4a/mp3/wav/ogg
# direto -> sem pre-conversao com ffmpeg. HTTP feito com urllib (stdlib): multipart montado a mao,
# zero dep nova. A chave vem de settings.groq_api_key (CP_GROQ_API_KEY no .env ou GROQ_API_KEY no env).
GROQ_URL = "https://api.groq.com/openai/v1/audio/transcriptions"
GROQ_MODEL = "whisper-large-v3-turbo"

# O que se dita neste app e prompt pra agente: nome de ferramenta, comando, caminho e sigla. Sao
# exatamente as palavras que a Whisper mais erra, porque nenhuma delas e portugues ("cp-send" sai
# "CP send", "Kimi K3" sai "QIMI K3"). O campo `prompt` da Whisper e vocabulario, nao instrucao:
# ele so enviesa a decodificacao pra grafia certa dessas palavras. Consertar aqui e melhor que
# consertar depois no LLM — a limpeza tem ordem explicita de PRESERVAR nome proprio como veio,
# entao o que a Whisper errou chega errado no fim.
#
# `language` fixo em pt: o audio deste app e sempre ditado do usuario. Sem ele a Whisper detecta o
# idioma sozinha e ja trocou frase curta com jargao ingles por transcricao em ingles.
IDIOMA = "pt"
# Vocabulario BASE: so termos do proprio app, que valem pra qualquer pessoa que o use. O que e de
# UMA pessoa (nome de projeto, de sessao, de cliente) entra pela config `ditado_vocabulario` e e
# somado a este.
VOCAB_BASE = (
    "cp-send, tmux, Claude Code, Codex, Kimi, Pi, Opus, Sonnet, Haiku, SSE, JSONL, backend, "
    "frontend, commit, merge request, deploy, endpoint, worktree, prompt, token"
)
# A Whisper le no maximo ~224 tokens de prompt e ignora calada o resto — uma lista que cresceu
# demais perderia justamente os termos do fim, sem aviso. Corta por caractere, com folga.
_VOCAB_MAX = 700


class TranscribeError(Exception):
    """Erro de transcricao com status HTTP pra o endpoint mapear direto."""
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def vocabulario() -> str:
    """Lista de termos que a Whisper deve grafar direito: a base do app mais o que o usuario
    acrescentou na tela. Truncada em _VOCAB_MAX pra nao cair no corte silencioso da API."""
    extra = (runtime_config.get("ditado_vocabulario") or "").strip()
    juntos = f"{VOCAB_BASE}, {extra}" if extra else VOCAB_BASE
    return juntos[:_VOCAB_MAX]


def build_multipart(filename: str, content: bytes, vocab: str = "") -> tuple[bytes, str]:
    """Monta um corpo multipart/form-data (model + response_format + language + prompt + file) e
    devolve (body, boundary). Separado da chamada de rede pra ser testavel sem tocar na Groq."""
    boundary = "----claudepocket" + secrets.token_hex(16)
    b = boundary.encode()
    parts: list[bytes] = []
    campos = [("model", GROQ_MODEL), ("response_format", "text"), ("language", IDIOMA)]
    if vocab:
        campos.append(("prompt", vocab))
    for name, value in campos:
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
    api_key = (runtime_config.get("groq_api_key") or "").strip()
    if not api_key:
        raise TranscribeError(503, "GROQ_API_KEY (ou CP_GROQ_API_KEY) nao configurada no backend")
    # Nome enviado a Groq: FIXO no servidor, so a extensao sanitizada (_safe_ext) — nunca o nome cru do
    # cliente, que interpolado no header Content-Disposition permitiria injecao de aspas/CRLF (partes/campos
    # extras no multipart). A Groq so usa a extensao pra detectar o formato. 'bin' (sem ext) -> webm.
    ext = _safe_ext(filename)
    if ext == "bin":
        ext = "webm"
    body, boundary = build_multipart(f"audio.{ext}", content, vocabulario())
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

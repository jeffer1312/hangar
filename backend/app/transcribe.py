import http.client
import logging
import secrets
import urllib.error
import urllib.request

from app.config import settings
from app import runtime_config
from app.uploads import _safe_ext

logger = logging.getLogger(__name__)

# Transcricao de audio via Groq (whisper-large-v3-turbo). Groq aceita webm/mp4/m4a/mp3/wav/ogg
# direto -> sem pre-conversao com ffmpeg. HTTP feito com urllib (stdlib): multipart montado a mao,
# zero dep nova. A chave vem de settings.groq_api_key (CP_GROQ_API_KEY no .env ou GROQ_API_KEY no env).
GROQ_URL = "https://api.groq.com/openai/v1/audio/transcriptions"
GROQ_MODEL = "whisper-large-v3-turbo"

# O que se dita neste app e prompt pra agente: nome de ferramenta, comando, caminho e sigla. Sao
# exatamente as palavras que a Whisper mais erra, porque nenhuma delas e portugues ("hangar-send" sai
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
    # `cp-send` fica junto do nome novo enquanto o comando antigo existir: é o que muita gente
    # ainda fala, e sem ele a Whisper devolve "CP send" — que a limpeza tem ordem de preservar.
    "hangar-send, cp-send, tmux, Claude Code, Codex, Kimi, Pi, Opus, Sonnet, Haiku, SSE, JSONL, backend, "
    "frontend, commit, merge request, deploy, endpoint, worktree, prompt, token"
)
# A Whisper le no maximo ~224 tokens de prompt e ignora calada o resto — uma lista que cresceu
# demais perderia justamente os termos do fim, sem aviso. Corta por caractere, com folga.
_VOCAB_MAX = 700
# Quanto sobra pro usuario depois da base. DERIVADO, nunca digitado a mao: mexer no VOCAB_BASE sem
# mexer aqui deixaria a tela aceitar um texto que o corte come depois — o silencio que este teto
# existe pra matar.
VOCAB_USUARIO_MAX = _VOCAB_MAX - len(VOCAB_BASE) - 2  # 2 = o ", " que junta as duas partes


class TranscribeError(Exception):
    """Erro de transcricao com status HTTP pra o endpoint mapear direto."""
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def vocabulario() -> str:
    """Lista de termos que a Whisper deve grafar direito: a base do app mais o que o usuario
    acrescentou na tela.

    O teto de verdade e na GRAVACAO (runtime_config._coagir recusa acima de VOCAB_USUARIO_MAX),
    porque e la que da pra falar com a pessoa: ela ve o erro na hora de salvar, em vez de descobrir
    meses depois que a Whisper nunca soube dos ultimos nomes que ela cadastrou. O corte aqui e a
    ULTIMA barreira (config escrita a mao no JSON, VOCAB_BASE que cresceu num upgrade) e por isso
    grita no log: repetir aqui o corte calado da API seria o mesmo defeito que este codigo evita."""
    extra = (runtime_config.get("ditado_vocabulario") or "").strip()
    juntos = f"{VOCAB_BASE}, {extra}" if extra else VOCAB_BASE
    if len(juntos) > _VOCAB_MAX:
        logger.warning(
            "vocabulario do ditado cortado: %d caracteres acima do teto de %d — os %d ultimos "
            "termos nao chegam na Whisper. Encurte o campo 'Palavras do seu ditado'.",
            len(juntos) - _VOCAB_MAX, _VOCAB_MAX, len(juntos) - _VOCAB_MAX,
        )
    return juntos[:_VOCAB_MAX]


def build_multipart(filename: str, content: bytes, vocab: str = "") -> tuple[bytes, str]:
    """Monta um corpo multipart/form-data (model + response_format + language + prompt + file) e
    devolve (body, boundary). Separado da chamada de rede pra ser testavel sem tocar na Groq."""
    boundary = "----hangar" + secrets.token_hex(16)
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
            "User-Agent": "hangar/1.0",
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

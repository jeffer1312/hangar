import http.client
import json
import logging
import os
import secrets
import tempfile
import threading
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from email.utils import parsedate_to_datetime
from pathlib import Path
from urllib.parse import urlparse

from app import atomico
from app import runtime_config
from app.uploads import _safe_ext

logger = logging.getLogger(__name__)

# Transcricao de audio por uma API compativel com a OpenAI. O servico padrao aceita
# webm/mp4/m4a/mp3/wav/ogg direto -> sem pre-conversao com ffmpeg. HTTP feito com urllib (stdlib):
# multipart montado a mao, zero dependencia nova. O nome interno da chave segue legado para nao
# invalidar CP_GROQ_API_KEY nem o runtime-config existente; a interface nao amarra o recurso a ele.
PADRAO_BASE_URL = "https://api.groq.com/openai/v1"
# O `turbo` nao e mais rapido neste uso: medido no mesmo audio, 1,07s contra 1,03s do grande num
# ditado de 42s. O que ele perde e sentido — chegou a inserir um "nao" que inverteu a frase — e
# pontuacao, que e justamente o trabalho que a limpeza depois tem que refazer.
GROQ_MODEL = "whisper-large-v3"

ELEVENLABS_URL = "https://api.elevenlabs.io/v1/speech-to-text"
ELEVENLABS_MODEL = "scribe_v2"
# (prazo de cada serviço, teto da lista). O cliente desiste aos 300 s. No ditado a limpeza ainda
# pode levar até 120 s depois; arquivo anexado e vídeo não passam por limpeza e são mais longos.
DICTATION_LIMITS = (60, 150)
FILE_LIMITS = (120, 240)
# A fala do vídeo roda dentro do /upload, que o cliente abandona aos 180 s.
VIDEO_LIMITS = (60, 120)
# Abaixo disto não vale abrir outra tentativa: ela estouraria o teto de qualquer jeito.
_MIN_ATTEMPT = 5
QUOTA_WAIT_429 = 3600
QUOTA_WAIT_402 = 86400
# O ElevenLabs manda cota esgotada até como 401, e 429 de concorrência não é cota.
_ELEVENLABS_QUOTA = {"quota_exceeded", "insufficient_credits"}
_ELEVENLABS_BUSY = {"concurrent_limit_exceeded", "system_busy"}
_KEYTERM_FORBIDDEN = set("<>{}[]\\")

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


@dataclass(frozen=True)
class Transcription:
    text: str
    provider: str
    aviso: str | None = None


class _Failure(Exception):
    """Uma tentativa sem texto, com o que a classificação precisa: status, corpo e cabeçalhos."""
    def __init__(self, error: TranscribeError, status: int | None = None, body: str = "",
                 headers=None, empty: bool = False):
        super().__init__(error.detail)
        self.error = error
        self.status = status
        self.body = body
        self.headers = headers
        # Respondeu 200 sem texto: o aviso não pode dizer que o serviço "não respondeu".
        self.empty = empty


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


def _keyterms(vocab: str) -> list[str]:
    """O vocabulário como `keyterms` do ElevenLabs, sem os termos que ele recusaria com 400."""
    out: list[str] = []
    for termo in vocab.split(","):
        termo = termo.strip()
        if (termo and len(termo) < 50 and len(termo.split()) <= 5
                and not (_KEYTERM_FORBIDDEN & set(termo)) and termo not in out):
            out.append(termo)
    return out


def _multipart(campos: list[tuple[str, str]], filename: str, content: bytes) -> tuple[bytes, str]:
    boundary = "----hangar" + secrets.token_hex(16)
    b = boundary.encode()
    parts: list[bytes] = []
    for name, value in campos:
        parts += [b"--" + b,
                  f'Content-Disposition: form-data; name="{name}"'.encode(),
                  b"", value.encode()]
    parts += [b"--" + b,
              f'Content-Disposition: form-data; name="file"; filename="{filename}"'.encode(),
              b"Content-Type: application/octet-stream", b"", content]
    parts += [b"--" + b + b"--", b""]
    return b"\r\n".join(parts), boundary


def build_multipart(filename: str, content: bytes, vocab: str = "",
                    model: str = GROQ_MODEL) -> tuple[bytes, str]:
    """Monta um corpo multipart/form-data (model + response_format + language + prompt + file) e
    devolve (body, boundary). Separado da chamada de rede pra ser testavel sem tocar na Groq."""
    campos = [("model", model), ("response_format", "json"), ("language", IDIOMA)]
    if vocab:
        campos.append(("prompt", vocab))
    return _multipart(campos, filename, content)


def _audio_ext(filename: str | None) -> str:
    # Nome enviado ao serviço: FIXO no servidor, só a extensão sanitizada (_safe_ext) — nunca o nome
    # cru do cliente, que interpolado no header Content-Disposition permitiria injeção de aspas/CRLF
    # (partes/campos extras no multipart). O serviço só usa a extensão pra detectar o formato.
    # 'bin' (sem ext) -> webm.
    ext = _safe_ext(filename)
    return "webm" if ext == "bin" else ext


def display_name(item: dict) -> str:
    """Nome do serviço na resposta, no aviso e na tela; vazio no item = derivado do tipo."""
    nome = (item.get("name") or "").strip()
    if nome:
        return nome
    if item.get("kind") == "elevenlabs":
        return "ElevenLabs"
    base = (item.get("base_url") or "").strip() or PADRAO_BASE_URL
    modelo = (item.get("model") or "").strip() or GROQ_MODEL
    return f"{urlparse(base).hostname or base} · {modelo}"


def configured_providers() -> list[dict]:
    """A lista salva, sem os itens que nem dá para tentar (JSON editado à mão)."""
    bruto = runtime_config.get("transcription_providers") or []
    if not isinstance(bruto, list):
        logger.warning("transcription_providers nao e uma lista; usando o servico unico")
        return []
    ok = []
    for item in bruto:
        if (isinstance(item, dict) and item.get("kind") in runtime_config.TRANSCRIPTION_KINDS
                and isinstance(item.get("id"), str) and item["id"]
                and isinstance(item.get("api_key", ""), str)
                and (item["kind"] != "elevenlabs" or item.get("api_key", "").strip())):
            ok.append(item)
        else:
            visivel = ({k: v for k, v in item.items() if k != "api_key"}
                       if isinstance(item, dict) else item)
            logger.warning("servico de transcricao ignorado (sem id, tipo ou chave): %r", visivel)
    return ok


def _openai_request(item: dict, content: bytes, ext: str) -> urllib.request.Request:
    base = (item.get("base_url") or "").strip().rstrip("/") or PADRAO_BASE_URL
    model = (item.get("model") or "").strip() or GROQ_MODEL
    body, boundary = build_multipart(f"audio.{ext}", content, vocabulario(), model)
    headers = {
        "Content-Type": f"multipart/form-data; boundary={boundary}",
        "User-Agent": "hangar/1.0",
    }
    if key := item.get("api_key", "").strip():
        headers["Authorization"] = f"Bearer {key}"
    return urllib.request.Request(
        f"{base}/audio/transcriptions", data=body, method="POST",
        headers=headers,
    )


def _elevenlabs_request(item: dict, content: bytes, ext: str) -> urllib.request.Request:
    model = (item.get("model") or "").strip() or ELEVENLABS_MODEL
    campos = [("model_id", model), ("language_code", IDIOMA), ("tag_audio_events", "false")]
    campos += [("keyterms", termo) for termo in _keyterms(vocabulario())]
    body, boundary = _multipart(campos, f"audio.{ext}", content)
    return urllib.request.Request(
        ELEVENLABS_URL, data=body, method="POST",
        headers={
            "xi-api-key": item["api_key"].strip(),
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "User-Agent": "hangar/1.0",
        },
    )


def _plain_text(raw: bytes) -> str:
    # response_format=text -> corpo e o texto puro. Achata espacos/quebras numa linha so.
    return " ".join(raw.decode("utf-8", "replace").split())


def _json_text(raw: bytes) -> str:
    try:
        text = json.loads(raw).get("text")
    except (ValueError, AttributeError):
        text = None
    if not isinstance(text, str):
        raise _Failure(TranscribeError(502, "resposta do servico de transcricao sem texto"), empty=True)
    text = " ".join(text.split())
    if not text:
        raise _Failure(TranscribeError(502, "resposta do serviço de transcrição sem texto"), empty=True)
    return text


def _send(req: urllib.request.Request, timeout: float, parse) -> str:
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read()
    except urllib.error.HTTPError as e:
        try:
            body: str | None = e.read().decode("utf-8", "replace")
        except (OSError, http.client.HTTPException):
            # ler o corpo de erro tambem e um read() de socket -> pode cair/timeout. Nao deixa
            # vazar cru (viraria 500); mantem o 502 com o codigo, sem o corpo.
            body = None
        detail = "(sem corpo)" if body is None else body[:300]
        raise _Failure(TranscribeError(502, f"servico de transcricao {e.code}: {detail}"),
                       e.code, body or "", e.headers)
    except (OSError, http.client.HTTPException) as e:
        # OSError cobre URLError (conexao) e TimeoutError/socket.timeout no read(); http.client cobre
        # IncompleteRead (conexao cai no meio da resposta). Sem isto, timeout no read vazaria como 500.
        raise _Failure(TranscribeError(502, f"falha ao contatar o servico de transcricao: {e}"))
    return parse(raw)


def _transcribe_legacy(content: bytes, filename: str | None) -> Transcription:
    """O serviço único das chaves `groq_api_key`/`transcription_*`: o caminho de antes da lista."""
    api_key = (runtime_config.get("groq_api_key") or "").strip()
    if not api_key:
        raise TranscribeError(503, "chave de transcricao nao configurada no backend")
    item = {"kind": "openai", "name": "", "api_key": api_key,
            "base_url": runtime_config.get("transcription_base_url") or "",
            "model": runtime_config.get("transcription_model") or ""}
    try:
        text = _send(_openai_request(item, content, _audio_ext(filename)), 120, _json_text)
    except _Failure as f:
        raise f.error from None
    return Transcription(text, display_name(item))


# --- Espera por cota, em disco: reiniciar o backend não volta a bater num serviço sem cota. ---

_STATE_LOCK = threading.Lock()


def _state_path() -> Path:
    return Path.home() / ".hangar" / "transcription-wait.json"


def _load_waits() -> dict:
    try:
        d = json.loads(_state_path().read_text(encoding="utf-8"))
    except OSError:
        return {}
    except ValueError:
        # A próxima espera grava por cima: sem este rastro, as esperas perdidas sumiam caladas.
        logger.warning("espera do servico de transcricao ilegivel; descartada")
        return {}
    return d if isinstance(d, dict) else {}


def _waiting_until(waits: dict, pid: str, now: float) -> float | None:
    w = waits.get(pid)
    until = w.get("until") if isinstance(w, dict) else None
    return float(until) if isinstance(until, (int, float)) and until > now else None


def _update_wait(pid: str, until: float | None, reason: str | None = None) -> None:
    """Grava (ou tira, com `until=None`) a espera de um serviço. Falha de disco vai ao log e não
    derruba o ditado: o texto já veio, ou o próximo serviço ainda pode trazê-lo."""
    with _STATE_LOCK:
        now = time.time()
        waits = _load_waits()
        waits = {k: v for k, v in waits.items() if _waiting_until(waits, k, now) is not None}
        if until is None:
            waits.pop(pid, None)
        else:
            waits[pid] = {"until": until, "reason": reason}
        path = _state_path()
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            fd, tmp = tempfile.mkstemp(dir=str(path.parent), suffix=".tmp")
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as fh:
                    json.dump(waits, fh, ensure_ascii=False)
                atomico.substituir(tmp, path)
            except BaseException:
                Path(tmp).unlink(missing_ok=True)
                raise
        except OSError:
            logger.exception("espera do servico de transcricao nao gravada")


def _retry_after(headers, now: float) -> float | None:
    valor = ((headers.get("retry-after") if headers is not None else None) or "").strip()
    if not valor:
        return None
    # `isdigit` sozinho aceita "²", que o `int` recusa fora do try.
    if valor.isascii() and valor.isdigit():
        return now + int(valor)
    try:
        return parsedate_to_datetime(valor).timestamp()
    except (TypeError, ValueError):
        return None


def _elevenlabs_code(body: str) -> str:
    try:
        detail = json.loads(body).get("detail")
    except (ValueError, AttributeError):
        return ""
    if isinstance(detail, dict):
        return str(detail.get("code") or detail.get("status") or "")
    return ""


def _classify(kind: str, f: _Failure, now: float) -> tuple[str, float | None]:
    """("transient" | "auth" | "quota" | "other", fim da espera). Só "quota" põe em espera."""
    if f.status is None or f.status >= 500:
        return "transient", None
    code = _elevenlabs_code(f.body) if kind == "elevenlabs" else ""
    if code in _ELEVENLABS_BUSY:
        return "transient", None
    if f.status in (402, 429) or code in _ELEVENLABS_QUOTA:
        fallback = QUOTA_WAIT_429 if f.status == 429 else QUOTA_WAIT_402
        return "quota", _retry_after(f.headers, now) or now + fallback
    if f.status in (401, 403):
        return "auth", None
    return "other", None


def _reason(name: str, verdict: str, f: _Failure) -> str:
    if verdict == "auth":
        return f"{name} recusou a chave ({f.status}); confira a chave desse serviço"
    if verdict == "quota":
        return f"{name} sem cota ({f.status}), em espera"
    if f.empty:
        return f"{name} deu resposta sem texto"
    if f.status is None:
        return f"{name} não respondeu"
    return f"{name} falhou ({f.status})"


def transcribe(content: bytes, filename: str | None) -> str:
    """Transcreve áudio e devolve UMA linha (vídeo anexado só quer o texto)."""
    return transcribe_with_provider(content, filename, VIDEO_LIMITS).text


def transcribe_with_provider(content: bytes, filename: str | None,
                             limits: tuple[float, float] = DICTATION_LIMITS) -> Transcription:
    """Percorre `transcription_providers` em ordem, pulando quem está em espera por cota. Lista
    vazia = o serviço único de sempre. Todos falhando: sobe o erro do PRIMEIRO tentado, que é o
    que a pessoa conserta, com o motivo curto de cada um dos seguintes no fim."""
    per_provider, budget = limits
    providers = configured_providers()
    if not providers:
        return _transcribe_legacy(content, filename)
    ext = _audio_ext(filename)
    waits = _load_waits()
    now = time.time()
    free = [p for p in providers if _waiting_until(waits, p["id"], now) is None]
    # Todos em espera: sem data do serviço a espera é palpite, e recusar o ditado sem tentar é pior.
    queue = free or providers
    deadline = time.monotonic() + budget
    first_error: TranscribeError | None = None
    reasons: list[str] = []
    for p in queue:
        remaining = deadline - time.monotonic()
        if remaining < _MIN_ATTEMPT:
            break
        name = display_name(p)
        elevenlabs = p["kind"] == "elevenlabs"
        try:
            if p["kind"] == "whisper_cpp":
                raise _Failure(TranscribeError(503, "whisper.cpp local requer o servidor Rust disponível"))
            req = (_elevenlabs_request if elevenlabs else _openai_request)(p, content, ext)
            text = _send(req, min(per_provider, remaining), _json_text)
        except _Failure as f:
            if first_error is None:
                first_error = TranscribeError(f.error.status, f"{name}: {f.error.detail}")
            verdict, until = _classify(p["kind"], f, time.time())
            if verdict == "quota":
                _update_wait(p["id"], until, f"sem cota ({f.status})")
            reasons.append(_reason(name, verdict, f))
            continue
        if p["id"] in waits:
            _update_wait(p["id"], None)
        # Espera pulada não vira aviso: aviso suprime o envio do mãos-livres nos clientes, e a
        # espera já aparece na tela de configuração.
        aviso = f"Transcrito pelo {name}: {'; '.join(reasons)}" if reasons else None
        return Transcription(text, name, aviso)
    if first_error is None:
        raise TranscribeError(504, "nenhum servico de transcricao respondeu a tempo")
    if len(reasons) > 1:
        raise TranscribeError(first_error.status, f"{first_error.detail} (depois: {'; '.join(reasons[1:])})")
    raise first_error


def providers_status() -> list[dict]:
    """Cada serviço da lista e, quando em espera por cota, até quando e por quê."""
    waits = _load_waits()
    now = time.time()
    out = []
    for p in configured_providers():
        until = _waiting_until(waits, p["id"], now)
        out.append({
            "id": p["id"],
            "name": display_name(p),
            "kind": p["kind"],
            "waiting_until": until,
            "reason": waits[p["id"]].get("reason") if until is not None else None,
        })
    return out

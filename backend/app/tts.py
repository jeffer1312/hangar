import hashlib
import http.client
import json
import os
import shlex
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

from app import runtime_config
from app.config import settings

# Sintese de voz. Mesma forma do transcribe.py: urllib da stdlib, sem dependencia nova, erro tipado
# com status HTTP pro endpoint mapear direto.

API = "https://api.elevenlabs.io/v1"
MODELO_PADRAO = "eleven_multilingual_v2"
# Teto DURO por requisicao, em caracteres — do MODELO EM USO (MODELO_PADRAO), nao do mais
# permissivo da conta: eleven_flash_v2_5 aceita 40k, mas a gente nao usa esse modelo. Confirmar
# aqui SE trocar MODELO_PADRAO, senao o servidor aceita texto que a ElevenLabs vai recusar.
TETO_CARACTERES = 10_000
VOZ_PADRAO = "ORgG8rwdAiMYRug8RJwR"
CACHE_SUBDIR = ".claude-pocket-tts"
TIMEOUT_LOCAL = 180        # segundos: motor local na CPU e lento; abaixo disso corta texto longo
RETENCAO_DIAS = 30


class TtsError(Exception):
    """Erro de sintese com status HTTP pra o endpoint mapear direto."""
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def _base_cache() -> Path:
    # Mesma raiz dos outros sidecars do app (chain.py, loop.py, pqueue.py). NAO e por sessao: o mesmo
    # trecho lido de duas sessoes diferentes deve reaproveitar o mesmo audio.
    return Path(settings.projects_dir).parent / CACHE_SUBDIR


def hash_de(texto: str, voz: str, provedor: str) -> str:
    """Chave do cache. Inclui voz e provedor: o mesmo texto em outra voz e outro audio."""
    bruto = f"{provedor}\x00{voz}\x00{texto}".encode("utf-8")
    return hashlib.sha256(bruto).hexdigest()


def extensao_de(audio: bytes) -> str:
    """wav ou mp3, pelos 4 primeiros bytes. O comando local pode devolver WAV (e o texto de ajuda
    da tela promete isso); a ElevenLabs sempre devolve mp3. Detectar pelo conteudo, nao pelo
    provedor, e o que permite `caminho_do_cache` achar o arquivo certo sem saber de antemao."""
    return "wav" if audio[:4] == b"RIFF" else "mp3"


def caminho_do_cache(h: str) -> Path:
    """Caminho do audio em cache. A extensao real pode ser .mp3 OU .wav (motor local) — quem le
    nao pode assumir uma so. Sem arquivo em nenhuma das duas, devolve o caminho .mp3 (nao existe;
    o chamador trata isso como cache miss)."""
    base = _base_cache()
    for ext in ("mp3", "wav"):
        p = base / f"{h}.{ext}"
        if p.exists():
            return p
    return base / f"{h}.mp3"


def corpo_elevenlabs(texto: str, modelo: str) -> bytes:
    """Corpo JSON da requisicao. Separado da rede pra ser testavel sem tocar no provedor.
    apply_text_normalization vai EXPLICITO: e ele que verbaliza numero, valor e data. Pegadinha
    registrada: ele nao vale no eleven_flash_v2_5, entao trocar pro modelo mais barato desliga isso."""
    return json.dumps({
        "text": texto,
        "model_id": modelo,
        "apply_text_normalization": "auto",
    }, ensure_ascii=False).encode("utf-8")


def _chave() -> str:
    k = (runtime_config.get("elevenlabs_api_key") or "").strip()
    if not k:
        raise TtsError(503, "chave da ElevenLabs nao configurada — abra Configurações do servidor")
    return k


def _pedir(caminho: str, dados: bytes | None = None) -> bytes:
    req = urllib.request.Request(
        f"{API}{caminho}",
        data=dados,
        method="POST" if dados is not None else "GET",
        headers={
            "xi-api-key": _chave(),
            "Content-Type": "application/json",
            "User-Agent": "claude-pocket/1.0",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return resp.read()
    except urllib.error.HTTPError as e:
        try:
            detalhe = e.read().decode("utf-8", "replace")[:300]
        except (OSError, http.client.HTTPException):
            detalhe = "(sem corpo)"
        raise TtsError(502, f"ElevenLabs {e.code}: {detalhe}")
    except (OSError, http.client.HTTPException) as e:
        raise TtsError(502, f"falha ao contatar a ElevenLabs: {e}")


def _voz_efetiva(voz: str) -> str:
    """Resolve a voz de fato usada: explicita > configurada > padrao. UM lugar so — hash_de e
    _baixar_elevenlabs tem que concordar em qual voz e essa, senao o cache fica preso na voz antiga
    quando elevenlabs_voice_id muda."""
    return voz or (runtime_config.get("elevenlabs_voice_id") or "").strip() or VOZ_PADRAO


def _baixar_elevenlabs(texto: str, voz: str) -> bytes:
    audio = _pedir(f"/text-to-speech/{voz}?output_format=mp3_44100_128",
                   corpo_elevenlabs(texto, MODELO_PADRAO))
    if not audio:
        raise TtsError(502, "ElevenLabs devolveu audio vazio")
    return audio


def _baixar_local(texto: str) -> bytes:
    cmd = (runtime_config.get("tts_local_cmd") or "").strip()
    if not cmd:
        raise TtsError(503, "comando de voz local nao configurado")
    # shell=False e argv por shlex: o texto vai pelo STDIN, nunca na linha de comando, entao nada do
    # que foi selecionado no chat pode virar argumento.
    try:
        partes = shlex.split(cmd)
    except ValueError as e:
        raise TtsError(503, f"comando de voz local mal formado (tts_local_cmd): {e}")
    try:
        p = subprocess.run(partes, input=texto.encode("utf-8"),
                           capture_output=True, timeout=TIMEOUT_LOCAL)
    except FileNotFoundError:
        raise TtsError(502, f"comando de voz local nao encontrado: {partes[0]}")
    except subprocess.TimeoutExpired:
        raise TtsError(504, f"comando de voz local passou de {TIMEOUT_LOCAL}s")
    if p.returncode != 0:
        erro = p.stderr.decode("utf-8", "replace")[:300] or f"codigo {p.returncode}"
        raise TtsError(502, f"comando de voz local falhou: {erro}")
    if not p.stdout:
        # Codigo 0 e saida vazia: sem isto o cache guardaria um arquivo de 0 byte e o player ficaria
        # mudo pra sempre, sem ninguem saber por que.
        raise TtsError(502, "comando de voz local nao escreveu audio na saida")
    return p.stdout


def _limpar_antigos(base: Path) -> None:
    """Higiene chamada na propria sintese, no padrao do prune_old dos uploads — sem agendador.
    Erro num arquivo nao derruba a varredura nem a sintese."""
    limite = time.time() - RETENCAO_DIAS * 86400
    try:
        for f in base.iterdir():
            try:
                if f.is_file() and f.stat().st_mtime < limite:
                    f.unlink()
            except OSError:
                pass
    except OSError:
        pass


def sintetizar(texto: str, voz: str, provedor: str) -> tuple[str, bool]:
    """Devolve (hash, veio_do_cache). Levanta TtsError."""
    if provedor not in ("elevenlabs", "local"):
        raise TtsError(400, f"provedor desconhecido: {provedor}")
    # Front nunca manda provider=local (nao ha campo pra isso na tela) — o motor local so e
    # alcancavel resolvendo aqui: sem chave da ElevenLabs mas com comando local configurado, usa ele
    # em vez de devolver "chave nao configurada" apontando pra tela onde a alternativa ja foi posta.
    if provedor == "elevenlabs" and not (runtime_config.get("elevenlabs_api_key") or "").strip() \
            and (runtime_config.get("tts_local_cmd") or "").strip():
        provedor = "local"
    # Voz resolvida ANTES do hash: "" so seria a voz efetiva por acidente, e um hash preso na string
    # vazia sobrevive a troca de elevenlabs_voice_id, servindo audio da voz antiga do cache calado.
    if provedor != "local":
        voz = _voz_efetiva(voz)
    h = hash_de(texto, voz, provedor)
    base = _base_cache()
    try:
        base.mkdir(parents=True, exist_ok=True)
    except OSError as e:
        raise TtsError(500, f"falha ao preparar a pasta de cache do audio: {e}")
    existente = caminho_do_cache(h)
    if existente.exists() and existente.stat().st_size > 0:
        return h, True

    if provedor == "local":
        audio = _baixar_local(texto)
    else:
        _chave()   # falha cedo, com 503, antes de qualquer trabalho
        audio = _baixar_elevenlabs(texto, voz)

    # Extensao pelo CONTEUDO (mp3 ou wav — ver extensao_de): o comando local pode devolver WAV.
    destino = base / f"{h}.{extensao_de(audio)}"
    # tmp+rename com o pid no nome: rede caindo no meio nao deixa arquivo truncado em cache pra
    # sempre, e dois pedidos simultaneos do mesmo trecho nao entrelacam bytes num nome fixo.
    tmp = base / f"{h}.{os.getpid()}.tmp"
    try:
        tmp.write_bytes(audio)
        tmp.replace(destino)
    except OSError as e:
        # Disco cheio/permissao negada aqui: o usuario ja PAGOU a chamada ao provedor (audio
        # baixado) — sem isto, a rota so captura TtsError e devolve 500 sem detail nenhum.
        raise TtsError(500, f"falha ao gravar o audio em cache: {e}")
    finally:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
    _limpar_antigos(base)
    return h, False


def listar_vozes() -> list[dict]:
    """Vozes da conta. Nunca lista chumbada: ela varia por conta e por assinatura."""
    dados = json.loads(_pedir("/voices").decode("utf-8", "replace"))
    return [{"id": v.get("voice_id"), "nome": v.get("name")} for v in dados.get("voices", [])]


def saldo() -> dict:
    """Consumo real da conta, em vez de aritmetica de preco na interface (que envelhece calada)."""
    d = json.loads(_pedir("/user/subscription").decode("utf-8", "replace"))
    return {"usados": d.get("character_count"), "limite": d.get("character_limit")}

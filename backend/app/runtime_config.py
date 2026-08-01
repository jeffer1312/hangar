import json
import os
import tempfile
import threading
from pathlib import Path
from typing import Any

from app.config import _backend_config_base, settings

# Configuração editável em RUNTIME.
#
# Até aqui as ~30 settings vinham só de env/.env: mudar a chave da Groq ou a retenção de anexos
# exigia editar arquivo no servidor e reiniciar o serviço — do celular, impossível. Esta camada é um
# JSON que fica POR CIMA do env: quem lê usa `get(campo)`, que devolve o override quando existe e o
# valor do env quando não.
#
# O que NÃO entra aqui, de propósito: porta, IP de bind, token de auth, chaves VAPID e segredos de
# sync/deploy. São coisas que ou exigem reiniciar o processo, ou dariam ao celular o poder de mudar
# a própria fechadura. Essas continuam só no env — a tela mostra o valor em leitura e diz qual
# variável mexer.
EDITAVEIS: dict[str, type] = {
    "groq_api_key": str,          # transcrição de áudio e de vídeo
    "upload_retention_days": int,  # dias que um anexo sobrevive
    "notify_finished": bool,
    "notify_dead": bool,
    "finish_min_seconds": int,
    "stall_seconds": int,
    "automations": bool,           # kill-switch das automações desatendidas
    "editor": str,
    "elevenlabs_api_key": str,     # sintese de voz (ouvir a selecao)
    "elevenlabs_voice_id": str,    # id da voz escolhida na conta
    "tts_local_cmd": str,          # comando externo opcional: texto no stdin, WAV no stdout
    "tts_max_chars": int,          # acima disso o app pede confirmacao antes de sintetizar
    # Ajustes de naturalidade da ElevenLabs (voice_settings), vindo de deslizantes na tela — o valor
    # e SEMPRE real (o slider nasce no padrao da ElevenLabs, nunca "vazio"). _coagir so tem tipo int
    # pra numero — sem float em EDITAVEIS — entao guardam o valor*100: tts_stability=50 -> 0.5 na
    # requisicao. Campo AUSENTE (usuario nunca tocou o slider) e campo IGUAL ao padrao da ElevenLabs
    # se comportam igual: tts.py:_ajustes_efetivos so manda a chave quando o valor FOGE do padrao.
    # tts_speed guarda 70-120 (velocidade 0.7x-1.2x); os outros tres guardam 0-100.
    "tts_stability": int,          # 0-100 = stability 0..1 (padrao ElevenLabs: 50 = 0.5)
    "tts_similarity_boost": int,   # 0-100 = similarity_boost 0..1 (padrao ElevenLabs: 75 = 0.75)
    "tts_style": int,              # 0-100 = style 0..1 (padrao ElevenLabs: 0)
    "tts_speed": int,              # 70-120 = speed 0.7..1.2 (padrao ElevenLabs: 100 = 1.0x)
    "llm_base_url": str,   # endpoint compativel com OpenAI (vazio = Groq)
    "llm_api_key": str,    # chave do provedor (so usada quando ha base_url proprio)
    "llm_model": str,      # nome do modelo (vazio = o padrao)
}

# Campos que NUNCA voltam inteiros pro cliente: o app devolve mascarado (gsk_••••1234) pra você
# conferir QUAL chave está lá sem poder copiá-la de volta.
SEGREDOS = {"groq_api_key", "elevenlabs_api_key", "llm_api_key"}

_ARQUIVO = "runtime-config.json"

# Serializa o read-modify-write: dois PATCH ao mesmo tempo liam o mesmo estado e o ultimo a
# gravar apagava a mudanca do outro, calado.
_LOCK = threading.Lock()


def _caminho() -> Path:
    return Path(_backend_config_base()) / _ARQUIVO


def _carregar() -> dict[str, Any]:
    try:
        with open(_caminho(), encoding="utf-8") as fh:
            d = json.load(fh)
        return d if isinstance(d, dict) else {}
    except (OSError, json.JSONDecodeError):
        # Arquivo ausente/corrompido não pode derrubar o backend: sem override, vale o env.
        return {}


def get(campo: str) -> Any:
    """Valor efetivo: override do arquivo, se houver; senão o do env."""
    if campo in EDITAVEIS:
        d = _carregar()
        if campo in d:
            return d[campo]
    return getattr(settings, campo, None)


def mascarar(valor: str) -> str:
    """Segredo em forma conferível, não copiável: mostra só o começo e o fim."""
    if not valor:
        return ""
    if len(valor) <= 8:
        return "•" * len(valor)
    return f"{valor[:4]}{'•' * 8}{valor[-4:]}"


def _coagir(campo: str, valor: Any) -> Any:
    """Converte o que veio do JSON pro tipo do campo. Levanta ValueError no que não dá."""
    tipo = EDITAVEIS[campo]
    if tipo is bool:
        if isinstance(valor, bool):
            return valor
        raise ValueError(f"{campo}: esperado true/false")
    if tipo is int:
        if isinstance(valor, bool) or not isinstance(valor, (int, float, str)):
            raise ValueError(f"{campo}: esperado número")
        try:
            n = int(valor)
        except (TypeError, ValueError):
            raise ValueError(f"{campo}: esperado número") from None
        if n < 0:
            raise ValueError(f"{campo}: não pode ser negativo")
        return n
    if not isinstance(valor, str):
        raise ValueError(f"{campo}: esperado texto")
    texto = valor.strip()
    if campo == "editor" and texto:
        # O editor vira argv[0] de um subprocess. Enquanto vinha so do .env, quem escolhia era o dono
        # da maquina; agora o celular escreve. Nome NU (sem barra, sem ..) mantem a escolha livre
        # (code, nvim, subl) e impede apontar pra um binario solto tipo /tmp/qualquer.sh.
        if "/" in texto or "\\" in texto or texto.startswith("-") or ".." in texto:
            raise ValueError("editor: use o nome do binario (ex: code), sem caminho")
    if campo == "llm_base_url" and texto and not (texto.startswith("http://") or texto.startswith("https://")):
        # Mesmo argumento do editor: antes so o dono da maquina escolhia o endpoint (env), agora o
        # celular escreve. Aceita vazio (volta ao padrao) ou uma URL http(s) de verdade.
        raise ValueError("llm_base_url: use vazio ou uma URL http(s)://")
    return texto


def aplicar(mudancas: dict[str, Any]) -> dict[str, Any]:
    """Grava os overrides. Ignora campo desconhecido (não deixa o cliente inventar setting).

    Escrita atômica (tmp + replace): um corte de energia no meio não deixa um JSON pela metade,
    que na próxima leitura viraria "sem override nenhum" — perder a configuração inteira calado.
    """
    with _LOCK:
        return _aplicar_travado(mudancas)


def _aplicar_travado(mudancas: dict[str, Any]) -> dict[str, Any]:
    atual = _carregar()
    for campo, valor in mudancas.items():
        if campo not in EDITAVEIS:
            continue
        # Segredo devolvido MASCARADO tem que ser reconhecido e ignorado. A checagem antiga era
        # "a string é só bullets?" — mas a máscara real é mista (gsk_••••••••1234), então NUNCA
        # batia: encostar no campo sobrescrevia a chave verdadeira pelo texto mascarado, sem volta.
        # Compara com a máscara do valor ATUAL, que é exatamente o que o cliente recebeu.
        if campo in SEGREDOS and isinstance(valor, str):
            efetivo = atual.get(campo) if campo in atual else getattr(settings, campo, "")
            if valor.strip() in {mascarar(efetivo or ""), ""} and efetivo:
                continue
        atual[campo] = _coagir(campo, valor)
    destino = _caminho()
    destino.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(destino.parent), suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(atual, fh, ensure_ascii=False, indent=2)
        os.replace(tmp, destino)
        # O arquivo guarda segredo (chave da Groq): 0600 como o .env, pra não ficar legível por
        # outro usuário da máquina. Falha de chmod não desfaz a gravação — o valor já está lá.
        try:
            os.chmod(destino, 0o600)
        except OSError:
            pass
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
    return atual


def estado() -> dict[str, Any]:
    """O que a tela mostra: valor efetivo de cada campo editável (segredo já mascarado) e se ele
    está vindo de um override ou do env."""
    overrides = _carregar()
    out: dict[str, Any] = {}
    for campo in EDITAVEIS:
        valor = get(campo)
        out[campo] = {
            "valor": mascarar(valor or "") if campo in SEGREDOS else valor,
            "definido": bool(valor) if campo in SEGREDOS else valor is not None,
            "origem": "app" if campo in overrides else "env",
        }
    return out

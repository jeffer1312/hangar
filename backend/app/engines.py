"""Motores de modelo alternativos: trocar o MOTOR sem trocar o CARRO.

A sessão continua no MESMO ~/.claude (skills, hooks, plugins, CLAUDE.md, statusline) e no mesmo
transcript .jsonl que o app já lê. O que muda é um punhado de variáveis de ambiente NO PROCESSO
daquela sessão — nada em disco, nada na conta logada.

Medido em 26/07/2026 (claude 2.1.220), contra Kimi Code e OmniRoute reais:
  - ANTHROPIC_BASE_URL + ANTHROPIC_AUTH_TOKEN bastam: não pede login, não pede aprovação de key.
  - ANTHROPIC_API_KEY (o que os projetos parecidos usam) dispara o prompt "detectei uma API key" e
    grava customApiKeyResponses no ~/.claude.json GLOBAL. Por isso aqui é AUTH_TOKEN, nunca API_KEY.
  - MAX_CONTEXT_TOKENS move a janela; AUTO_COMPACT_WINDOW (o que a doc da Moonshot manda) é inerte.

STDLIB PURA de propósito: o wrapper de shell importa este módulo com o `python3` do sistema, fora do
venv do backend (ver scripts/cp-engine). Um `from app.config import …` aqui puxaria pydantic e
quebraria o terminal, deixando só o app funcionando. Há teste que trava isso.
"""
import ipaddress
import json
import os
import re
import tempfile
import threading
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

# Campos aceitos. Qualquer outro é descartado: o cliente não inventa campo (mesma regra do
# runtime_config.EDITAVEIS). Não há campo de "esforço": medido que o provedor pode ignorar o pedido
# do CC (no OmniRoute quem manda é o sufixo do id do modelo), então seria um controle que não controla.
_CAMPOS: dict[str, type] = {
    "label": str,
    "base_url": str,
    "api_key": str,
    "model": str,
    "context_window": int,   # -> CLAUDE_CODE_MAX_CONTEXT_TOKENS
    "vision": bool,          # informativo; vem do /v1/models do provedor
    "tool_search": bool,     # ver env_de
}
_OBRIGATORIOS = ("base_url", "api_key", "model")

# Serializa read-modify-write: dois PUT ao mesmo tempo liam o mesmo estado e o último a gravar
# apagava o motor do outro. Protege só ESTE processo (o backend); o cp-engine apenas lê.
_LOCK = threading.Lock()

_NOME_OK = re.compile(r"^[a-z0-9][a-z0-9_-]{0,31}$")
_PROIBIDO_NO_VALOR = "\n\r\x00"


def caminho() -> Path:
    # Fixo em ~/.claude: motor é ortogonal a perfil. Derivar de CLAUDE_CONFIG_DIR faria o cp-engine
    # de um terminal com perfil alternativo ver ZERO motores enquanto o app mostra dois.
    return Path(os.environ.get("CP_ENGINES_FILE") or (Path.home() / ".claude" / "engines.json"))


def listar() -> dict[str, dict[str, Any]]:
    """Motores gravados, com a api_key INTEIRA. Quem devolve ao cliente mascara na borda HTTP."""
    try:
        with open(caminho(), encoding="utf-8") as fh:
            d = json.load(fh)
        return d if isinstance(d, dict) else {}
    except (OSError, json.JSONDecodeError):
        # Ausente/corrompido não derruba backend nem terminal: sem motor, vale o de hoje (Anthropic).
        return {}


def _host_local(host: str) -> bool:
    try:
        ip = ipaddress.ip_address(host)
    except ValueError:
        return host in ("localhost", "localhost.localdomain")
    return ip.is_loopback or ip.is_private


def validar_base_url(url: str) -> str:
    p = urlparse(url)
    if p.scheme not in ("http", "https") or not p.hostname:
        raise ValueError("base_url: use uma URL http(s) completa")
    if p.scheme == "http" and not _host_local(p.hostname):
        # A api_key vai no header Authorization: em http para host público ela atravessa a rede em
        # claro. Loopback/rede privada segue liberado — é o caso do proxy tradutor local.
        raise ValueError("base_url: use https (http só para loopback ou rede privada)")
    return url.rstrip("/")


def _normalizar(nome: str, dados: dict[str, Any]) -> dict[str, Any]:
    if not _NOME_OK.match(nome or ""):
        raise ValueError("nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)")
    out: dict[str, Any] = {}
    for campo, tipo in _CAMPOS.items():
        if campo not in dados or dados[campo] is None:
            continue
        valor = dados[campo]
        if tipo is bool:
            if not isinstance(valor, bool):
                raise ValueError(f"{campo}: esperado true/false")
            out[campo] = valor
        elif tipo is int:
            if isinstance(valor, bool) or not isinstance(valor, (int, float, str)):
                raise ValueError(f"{campo}: esperado número")
            try:
                n = int(valor)
            except (TypeError, ValueError):
                raise ValueError(f"{campo}: esperado número") from None
            if n <= 0:
                raise ValueError(f"{campo}: deve ser maior que zero")
            out[campo] = n
        else:
            if not isinstance(valor, str):
                raise ValueError(f"{campo}: esperado texto")
            texto = valor.strip()
            # Uma linha por variável é CONTRATO com o shell: `cp-engine --env` imprime CHAVE=VALOR e o
            # claude-engine dá export nisso. Um \n aqui exportaria variável arbitraria (PATH, LD_*).
            if any(c in texto for c in _PROIBIDO_NO_VALOR):
                raise ValueError(f"{campo}: sem quebra de linha nem caractere nulo")
            if texto:
                out[campo] = texto
    for campo in _OBRIGATORIOS:
        if not out.get(campo):
            raise ValueError(f"{campo}: obrigatório")
    out["base_url"] = validar_base_url(out["base_url"])
    out.setdefault("label", nome)
    return out


def salvar(nome: str, dados: dict[str, Any]) -> dict[str, Any]:
    """Grava um motor. Campo desconhecido é descartado; inválido levanta ValueError."""
    registro = _normalizar(nome, dados)
    with _LOCK:
        atual = listar()
        atual[nome] = registro
        _gravar(atual)
    return registro


def remover(nome: str) -> bool:
    with _LOCK:
        atual = listar()
        if nome not in atual:
            return False
        del atual[nome]
        _gravar(atual)
    return True


def _gravar(tudo: dict[str, Any]) -> None:
    # Escrita atômica (tmp + replace): um corte no meio não deixa JSON pela metade, que na próxima
    # leitura viraria "nenhum motor configurado" — perder a config inteira, calado.
    destino = caminho()
    destino.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(destino.parent), suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(tudo, fh, ensure_ascii=False, indent=2)
        os.replace(tmp, destino)
        try:
            os.chmod(destino, 0o600)
        except OSError:
            # Guarda a key do provedor; falha de chmod não desfaz a gravação (o valor já está lá).
            pass
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def env_de(nome: str) -> dict[str, str]:
    """Variáveis de ambiente que fazem uma sessão rodar neste motor.

    KeyError no motor inexistente de propósito: env vazio faria a sessão subir na conta Anthropic
    ACHANDO que é o motor escolhido — o pior tipo de falha, a silenciosa.
    """
    e = listar()[nome]
    modelo = e["model"]
    env = {
        # Marca lida do /proc/<pid>/environ para descobrir o motor de uma sessão viva (Task 5).
        "CP_ENGINE": nome,
        "ANTHROPIC_BASE_URL": e["base_url"],
        "ANTHROPIC_AUTH_TOKEN": e["api_key"],
        # Os 6 andam juntos: faltar um faz subagent/background falhar sem mensagem clara.
        "ANTHROPIC_MODEL": modelo,
        "ANTHROPIC_DEFAULT_OPUS_MODEL": modelo,
        "ANTHROPIC_DEFAULT_SONNET_MODEL": modelo,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": modelo,
        "ANTHROPIC_DEFAULT_FABLE_MODEL": modelo,
        "CLAUDE_CODE_SUBAGENT_MODEL": modelo,
    }
    if e.get("context_window"):
        # MAX_CONTEXT_TOKENS, nao AUTO_COMPACT_WINDOW: medido nos dois provedores, a segunda nao move
        # a janela (o /context seguia em 200k) e a primeira move. Sem isto, um modelo de 256k/500k
        # compacta em ~167k — capacidade jogada fora, calado.
        env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] = str(e["context_window"])
    if e.get("tool_search") is not True:
        # Default desligado por FAIL-SAFE, não por medição: a doc da Moonshot diz que o endpoint do
        # Kimi ainda não suporta Tool Search, e um erro no meio do turno é pior que uma ferramenta a
        # menos. Provedor que suportar liga com "tool_search": true.
        env["ENABLE_TOOL_SEARCH"] = "false"
    return env

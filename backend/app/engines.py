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
import logging
import os
import re
import tempfile
import threading
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

_log = logging.getLogger(__name__)

# Campos aceitos. Qualquer outro é descartado: o cliente não inventa campo (mesma regra do
# runtime_config.EDITAVEIS). Não há campo de "esforço": medido que o provedor pode ignorar o pedido
# do CC (no OmniRoute quem manda é o sufixo do id do modelo), então seria um controle que não controla.
_CAMPOS: dict[str, type] = {
    "label": str,
    "base_url": str,
    "api_key": str,
    "model": str,
    "subagent_model": str,   # -> CLAUDE_CODE_SUBAGENT_MODEL; vazio = mesmo modelo principal
    "context_window": int,   # -> CLAUDE_CODE_MAX_CONTEXT_TOKENS
    "vision": bool,          # informativo; vem do /v1/models do provedor
    # Capacidades do harness. TODAS são positivas ("true = ligado") mesmo quando a env var do Claude
    # Code é negativa (DISABLE_*): misturar as duas polaridades num formulário faz o usuário marcar
    # uma caixa pra desligar algo. env_de() faz a tradução; a UI só mostra capacidade.
    "tool_search": bool,                  # ver env_de
    "bundled_skills": bool,               # ver env_de
    "experimental_betas": bool,           # ver env_de
    "prompt_caching": bool,               # ver env_de
    "adaptive_thinking": bool,            # ver env_de
    "gateway_model_discovery": bool,      # ver env_de
    "fine_grained_tool_streaming": bool,  # ver env_de
    "auto_compact_window": int,           # -> CLAUDE_CODE_AUTO_COMPACT_WINDOW
    "max_output_tokens": int,             # -> CLAUDE_CODE_MAX_OUTPUT_TOKENS
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


def _estado() -> tuple[dict[str, Any], bool]:
    """Lê o arquivo bruto. Devolve (dict, corrompido).

    Ausente é o estado NORMAL — ninguém configurou motor ainda — e fica calado (corrompido=False):
    logar aqui incomodaria em todo boot/tick do SSE de quem nunca usou a feature. Presente mas
    ilegível (JSON quebrado, ou não é um objeto) é ANORMAL: é o achado do review — um typo no
    hand-edit quebra o arquivo, listar() volta {} igual a "nunca configurou nada", o usuário re-adiciona
    um motor achando que é a primeira vez, e a gravação por cima apaga os outros motores e suas
    keys, calado. Loga aqui pra não repetir; quem grava (salvar/remover) usa o corrompido=True pra
    recusar a escrita.
    """
    p = caminho()
    try:
        texto = p.read_text(encoding="utf-8")
    except FileNotFoundError:
        return {}, False
    except OSError as e:
        _log.warning("engines.json (%s) não pôde ser lido: %s — tratando como vazio nesta leitura, "
                     "mas recusando sobrescrever até alguém corrigir ou mover o arquivo.", p, e)
        return {}, True
    try:
        d = json.loads(texto)
    except ValueError as e:  # inclui json.JSONDecodeError
        _log.warning("engines.json (%s) existe mas não é JSON válido (%s) — tratando como vazio "
                     "nesta leitura, mas recusando sobrescrever até alguém corrigir ou mover o "
                     "arquivo.", p, e)
        return {}, True
    if not isinstance(d, dict):
        _log.warning("engines.json (%s) não é um objeto JSON — tratando como vazio nesta leitura, "
                     "mas recusando sobrescrever até alguém corrigir ou mover o arquivo.", p)
        return {}, True
    return d, False


def listar() -> dict[str, dict[str, Any]]:
    """Motores gravados, com a api_key INTEIRA. Quem devolve ao cliente mascara na borda HTTP."""
    d, _corrompido = _estado()
    # O nome vira parte de um `$SHELL -c` (registry.py monta `cp-engine --exec {nome} -- ...`).
    # salvar() já barra nome fora do padrão, mas o arquivo é hand-editável (0600, mas ainda assim);
    # pular o registro corrupto em vez de derrubar a lista inteira — um motor ruim não pode tirar
    # os outros do ar.
    return {nome: v for nome, v in d.items() if isinstance(nome, str) and _NOME_OK.match(nome)}


def arquivo_corrompido() -> bool:
    """True quando engines.json existe mas não pôde ser lido como JSON de motores.

    listar() devolve {} tanto pra isto quanto pra "nunca configurou nada" (de propósito — não pode
    derrubar sessão nem o tick do SSE por um hand-edit ruim). A API usa este sinal à parte pra tela
    parar de dizer "nenhum motor ainda" quando na verdade há um arquivo quebrado escondendo motores
    reais (o item 1 do review).
    """
    return _estado()[1]


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


def _atual_ou_recusa() -> dict[str, dict[str, Any]]:
    # Chamado só de dentro do _LOCK por salvar()/remover(). Corrompido = recusa a escrita: perder
    # esta gravação é recuperável (usuário tenta de novo depois de corrigir o arquivo); perder o
    # arquivo inteiro sobrescrito com {} + um registro novo não é (o achado do item 1 do review).
    d, corrompido = _estado()
    if corrompido:
        raise ValueError(
            f"engines.json ({caminho()}) existe mas está corrompido (JSON inválido); corrija-o à "
            "mão ou mova-o antes de salvar/remover — gravar por cima agora apagaria os outros "
            "motores e as keys deles."
        )
    return {nome: v for nome, v in d.items() if isinstance(nome, str) and _NOME_OK.match(nome)}


def salvar(nome: str, dados: dict[str, Any]) -> dict[str, Any]:
    """Grava um motor. Campo desconhecido é descartado; inválido levanta ValueError."""
    registro = _normalizar(nome, dados)
    with _LOCK:
        atual = _atual_ou_recusa()
        atual[nome] = registro
        _gravar(atual)
    return registro


def remover(nome: str) -> bool:
    with _LOCK:
        atual = _atual_ou_recusa()
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


def _inteiro_positivo(campo: str, valor: Any) -> int:
    """Normaliza um campo int na hora de virar env var, ou estoura.

    `_normalizar` já rejeita não-numérico e `<= 0` no SAVE, mas o engines.json é hand-editável: um
    valor negativo lá passava direto e virava `CLAUDE_CODE_MAX_CONTEXT_TOKENS=-1000` — o Claude Code
    não valida isso, então a sessão subia com uma janela absurda e a falha aparecia longe da causa.
    Falhar aqui é a mesma escolha que _PROIBIDO_NO_VALOR faz para os campos string: recusar o
    arquivo envenenado em vez de exportar um valor que ninguém mais confere.
    """
    try:
        n = int(valor)
    except (TypeError, ValueError):
        raise ValueError(f"{campo}: esperado número") from None
    if n <= 0:
        raise ValueError(f"{campo}: deve ser maior que zero")
    return n


def env_de(nome: str) -> dict[str, str]:
    """Variáveis de ambiente que fazem uma sessão rodar neste motor.

    KeyError no motor inexistente de propósito: env vazio faria a sessão subir na conta Anthropic
    ACHANDO que é o motor escolhido — o pior tipo de falha, a silenciosa.

    ValueError se qualquer valor contém caractere proibido (\\n, \\r, \\x00): contrato com o shell
    que `cp-engine --env` exporta (uma linha por variável). Se engines.json foi hand-editado ou
    corrompido, rejeitamos em vez de silenciar.
    """
    e = listar()[nome]

    # Valida shell-safety: uma linha por variável é o contrato com o shell. Rejeita se o JSON
    # contém um valor que já violou a invariante (hand-edited file, corrupted write recovered by
    # another tool, etc). Reutiliza _PROIBIDO_NO_VALOR para uma única fonte de verdade.
    for campo in ("api_key", "model", "base_url", "subagent_model"):
        valor = e.get(campo, "")
        if isinstance(valor, str) and any(c in valor for c in _PROIBIDO_NO_VALOR):
            raise ValueError(f"{campo}: contém caractere proibido (quebra de linha ou nulo)")

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
        # Subagentes fazem muita busca mecânica; um modelo mais barato aí é dinheiro de verdade.
        # Vazio (campo ausente) cai no mesmo modelo principal — nunca uma env var vazia.
        "CLAUDE_CODE_SUBAGENT_MODEL": e.get("subagent_model") or modelo,
    }
    if e.get("context_window"):
        # MAX_CONTEXT_TOKENS, nao AUTO_COMPACT_WINDOW: medido nos dois provedores, a segunda nao move
        # a janela (o /context seguia em 200k) e a primeira move. Sem isto, um modelo de 256k/500k
        # compacta em ~167k — capacidade jogada fora, calado.
        env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] = str(_inteiro_positivo("context_window", e["context_window"]))
    if e.get("bundled_skills") is not True:
        # Default desligado por MEDIÇÃO: a skill empacotada `claude-api` não tem SKILL.md na raiz, e
        # invocá-la injeta os 64 arquivos dela de uma vez — medido em 847.630 chars / 206.553 tokens
        # num único turno (12% -> 67% da janela numa sessão gpt-5.6-sol de 372k). O gatilho dela é
        # amplo ("o prompt cita Claude/Anthropic em qualquer forma"), então um "qual modelo você é?"
        # basta. Na conta Anthropic isso passa (janela maior + poda server-side via
        # context_management, que nenhum provedor de terceiro implementa); num motor, mata a sessão.
        # Só as BUNDLED caem: plugin e ~/.claude/skills/ seguem intactas.
        env["CLAUDE_CODE_DISABLE_BUNDLED_SKILLS"] = "1"
    if e.get("tool_search") is not True:
        # Default desligado por FAIL-SAFE, não por medição: a doc da Moonshot diz que o endpoint do
        # Kimi ainda não suporta Tool Search, e um erro no meio do turno é pior que uma ferramenta a
        # menos. Provedor que suportar liga com "tool_search": true.
        # Confirmado pela doc depois: o próprio Claude Code já desliga tool search quando
        # ANTHROPIC_BASE_URL aponta pra host não-first-party, "since most proxies do not forward
        # tool_reference blocks". Manter explícito não custa e vale pro gateway que se anuncia
        # first-party.
        env["ENABLE_TOOL_SEARCH"] = "false"
    if e.get("experimental_betas") is not True:
        # Default desligado: campos beta que o CC manda sozinho (context_management, campos beta de
        # tool) fazem o upstream de terceiro devolver `400 Extra inputs are not permitted`. A doc do
        # gateway lista essa var como a remediação do lado do cliente.
        # ATENÇÃO à dependência: com isto ligado, tool search fica off e ENABLE_TOOL_SEARCH NÃO
        # consegue sobrepor — por isso a UI desabilita aquele toggle quando este está desmarcado.
        # NÃO cobre `output_config`/effort: a doc não lista remediação de cliente pra esse campo.
        env["CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS"] = "1"
    if e.get("prompt_caching") is False:
        # Default LIGADO (só desliga com false explícito): cache é economia, e num gateway ele já
        # degrada sozinho e calado — "if the gateway rejects the cache breakpoint, Claude Code
        # retries without it and leaves that block uncached for the rest of the conversation".
        # Desligar de propósito só faz sentido em provedor que cobra o cache mais caro que o miss.
        env["DISABLE_PROMPT_CACHING"] = "1"
    if e.get("adaptive_thinking") is False:
        # Default LIGADO. Desligar thinking é destrutivo em alguns provedores: a doc da Kimi diz que
        # "disabling thinking routes both K3 and K2.7 Code to K2.6" — ou seja, rebaixa o modelo
        # calado. Só marque false se o upstream devolver 400 citando `thinking`/`adaptive`.
        env["CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"] = "1"
    if e.get("gateway_model_discovery") is True:
        # Opt-in: consulta /v1/models do gateway e popula o picker /model. Fora por padrão porque é
        # uma chamada extra a um endpoint que nem todo gateway expõe.
        env["CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"] = "1"
    if e.get("fine_grained_tool_streaming") is True:
        # Opt-in: o CC desliga por padrão atrás de base URL custom.
        env["CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING"] = "1"
    for campo, var in (("auto_compact_window", "CLAUDE_CODE_AUTO_COMPACT_WINDOW"),
                       ("max_output_tokens", "CLAUDE_CODE_MAX_OUTPUT_TOKENS")):
        if e.get(campo):
            # Sem valor = não inventa a var (o CC usa o default dele).
            env[var] = str(_inteiro_positivo(campo, e[campo]))
    return env

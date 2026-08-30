"""Catálogo de modelos do Codex: a fonte da lista na tela de ABERTURA, onde ainda não há sessão.

Por que não é como nenhum dos outros três: o `~/.codex/config.toml` guarda só o modelo escolhido
(`model = "..."`), nunca a lista — então não há o caminho do Kimi. E não existe `codex
--list-models`, então também não há o caminho do Pi. O que existe é o `model/list` do app-server, e
medido em 30/08/2026 (codex-cli 0.151.0) ele responde no modo **stdio**, sem `--listen`, sem thread
aberta e sem sessão viva, em 0,78s. É a mesma fonte que a folha da sessão viva usa
(`CodexAdapter.list_models`), só que por um processo efêmero em vez do app-server do pane.

Os `efforts` de cada modelo vêm daqui e não de uma lista no código porque **variam por modelo**
(medido: `gpt-5.6-sol` aceita `ultra`, `gpt-5.6-luna` não; `gpt-5.5` também não aceita `max`) — a
mesma lição que o Pi já tinha ensinado.

Cache pelo motivo do pi_catalog: é subprocess, e a lista muda de mês em mês.
"""
import json
import shutil
import subprocess
import threading
import time

# Mesmo clientInfo do handshake da sessão viva (docs/codex-app-server-contract.md): uma identidade
# só do hangar no protocolo.
from app.adapters.codex.lancador import CLIENT_INFO

_TTL = 600.0
_cache: tuple[float, list[dict]] | None = None
# Sobe um processo e faz duas chamadas: mediana de 0,8s. O teto é folgado porque o app-server lê o
# config.toml e carrega plugins na largada.
_TIMEOUT = 30.0


class CodexAusente(RuntimeError):
    """`codex` não está no PATH deste backend — não é falha do comando, é ausência do binário."""


def _binario() -> str:
    """Caminho do `codex`, resolvido — nunca o nome cru no argv. Mesmo motivo do pi_catalog: no
    Windows o CreateProcess só completa `.exe`, e o `which` aplica o PATHEXT."""
    exe = shutil.which("codex")
    if exe is None:
        raise CodexAusente("nao achei o executavel `codex` no PATH deste servidor — instale o "
                           "Codex CLI ou ajuste o PATH do backend")
    return exe


def parse(result: dict) -> list[dict]:
    """A resposta do `model/list` no formato da tela. Estoura se não sobrar modelo nenhum."""
    out: list[dict] = []
    for m in result.get("data") or []:
        # `hidden` é o provedor dizendo "não ofereça este": oferecer faria a sessão nascer num id
        # que o plano do usuário não atende, e a falha só apareceria no primeiro turno.
        if not isinstance(m, dict) or m.get("hidden") or not m.get("model"):
            continue
        out.append({
            "id": m["model"],
            "name": m.get("displayName") or m["model"],
            "desc": m.get("description") or "",
            "efforts": [e.get("reasoningEffort") for e in (m.get("supportedReasoningEfforts") or [])
                        if isinstance(e, dict) and e.get("reasoningEffort")],
            # `default_effort` é o mesmo campo que o catálogo do Kimi já manda — a tela lê os dois
            # pelo mesmo `ModelOption`. O `isDefault` do provedor NÃO entra: quem decide o padrão
            # desta máquina é o `model` do `~/.codex/config.toml`, e mostrar o outro como "padrão"
            # apontaria pro modelo errado.
            "default_effort": m.get("defaultReasoningEffort"),
        })
    if not out:
        # Zero modelo com rc=0 é falha do provedor (login vencido, versão que mudou o schema), não
        # "seu plano não tem modelo". Levanta pra virar o 502 que a rota já sabe dar, e o caller
        # NÃO cacheia: senão o erro duraria 10 min depois de o Codex voltar.
        raise RuntimeError("codex app-server nao devolveu modelo nenhum em model/list")
    return out


def _perguntar() -> dict:
    """Sobe um app-server em stdio, pergunta o `model/list` e devolve o `result`.

    NÃO dá pra usar `subprocess.run(input=...)`: medido em 30/08/2026, com o stdin fechado junto
    com a entrada o app-server responde o `initialize` e SAI (rc=0, 0,25s) sem chegar no segundo
    pedido — a lista voltava vazia com sucesso aparente. O canal fica aberto até a resposta chegar.

    `encoding` explícito pelo mesmo motivo dos outros: `text=True` sozinho decodifica pelo locale
    (cp1252 no Windows), e os rótulos de modelo não são só ASCII.
    """
    proc = subprocess.Popen(
        [_binario(), "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace", bufsize=1,
    )
    # O teto de tempo mata o processo em vez de embrulhar o `readline`: um app-server que trava sem
    # fechar o stdout deixaria a leitura pendurada pra sempre, e é o pane de quem usa que paga.
    carrasco = threading.Timer(_TIMEOUT, proc.kill)
    carrasco.daemon = True
    carrasco.start()
    try:
        proc.stdin.write("\n".join([
            json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": {"clientInfo": CLIENT_INFO, "capabilities": None}}),
            json.dumps({"jsonrpc": "2.0", "id": 2, "method": "model/list", "params": {}}),
        ]) + "\n")
        proc.stdin.flush()
        for linha in proc.stdout:
            try:
                msg = json.loads(linha)
            except ValueError:
                continue  # o app-server também escreve notificação e log; linha torta não é erro
            if isinstance(msg, dict) and msg.get("id") == 2 and isinstance(msg.get("result"), dict):
                return msg["result"]
        # Mata ANTES de ler o stderr: o `read()` vai até o EOF, e um processo ainda vivo com o
        # stderr aberto penduraria a rota justamente no caminho de falha.
        proc.kill()
        raise RuntimeError((proc.stderr.read() or "").strip()[-500:]
                           or "codex app-server nao respondeu model/list")
    finally:
        carrasco.cancel()
        proc.kill()
        proc.wait()


def listar(fresco: bool = False) -> list[dict]:
    global _cache
    if _cache and not fresco and time.monotonic() - _cache[0] < _TTL:
        return _cache[1]
    modelos = parse(_perguntar())
    _cache = (time.monotonic(), modelos)
    return modelos


def checar_escolha(model: str | None, effort: str | None) -> None:
    """Recusa (ValueError) modelo fora do catálogo, ou nível que AQUELE modelo não lista.

    `model_args` só valida a FORMA do nível — não pode ter lista fechada, porque os níveis variam
    por modelo. Sem esta checagem, `--effort ultra` num `gpt-5.5` nasce a sessão e o binário
    **descarta o nível calado** (medido em 30/08/2026: ele não morre, segue com o dele), ou seja, o
    app reportaria sucesso sobre uma escolha que não valeu. Mesma doutrina do `engine_model_set`:
    recusar aqui em vez de deixar a falha aparecer só no turno.

    Nível sem modelo não é checável (o modelo então é o do `~/.codex/config.toml`, que este
    catálogo não diz qual é) e passa.
    """
    if model is None:
        return
    for m in listar():
        if m["id"] == model:
            if effort is not None and effort not in m["efforts"]:
                raise ValueError(f"nivel fora do suporte de {model}: {effort!r} "
                                 f"(use um de {', '.join(m['efforts']) or 'nenhum'})")
            return
    raise ValueError(f"modelo fora do catalogo do Codex: {model}")

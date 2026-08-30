"""Perguntar UMA coisa ao Codex sem sessão viva: um app-server efêmero em stdio.

Existe porque duas telas precisam da mesma máquina e nenhuma delas tem pane: o catálogo de modelos
da abertura (`app/codex_models.py`) e a cota por credencial (`app/cotas.py`). O app-server do pane,
quando existe, é do adapter — este aqui sobe, pergunta e morre.

Medido em 30/08/2026 (codex-cli 0.151.0): `codex app-server` **sem** `--listen` fala JSON-RPC por
linha no stdout, aceita `model/list` (0,78s) e `account/rateLimits/read` (1,2s) sem thread aberta,
sem pane e sem sessão. A credencial que ele usa é a do `~/.codex/auth.json` — que é justamente o
que o painel de cotas quer: uma fonte por CREDENCIAL, não por sessão.

Só stdlib, de propósito: o `scripts/hangar-codex-tui` roda no `python3` do sistema e pode um dia
precisar disto.
"""
import json
import os
import shutil
import subprocess
import threading
from pathlib import Path

# Sobe um processo e faz duas chamadas: mediana de 0,8s a 1,2s. O teto é folgado porque o
# app-server lê o config.toml e carrega plugins na largada.
_TIMEOUT = 30.0

# Mesmo clientInfo do handshake da sessão viva (docs/codex-app-server-contract.md): uma identidade
# só do hangar no protocolo.
from app.adapters.codex.lancador import CLIENT_INFO


def home() -> Path:
    """A pasta do Codex (`CODEX_HOME`, ou `~/.codex`) — onde moram a credencial e as conversas.

    Mora aqui porque este é o módulo compartilhado do Codex. A mesma expressão está copiada em
    `costs_sources`, `archive_providers` e `agentes_sync._codex_dir` (que tem outra assinatura, com
    `home` explícito) — código novo usa esta; converter as três é mudança de outro assunto.
    """
    return Path(os.environ.get("CODEX_HOME") or (Path.home() / ".codex"))


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


def perguntar(metodo: str, timeout: float = _TIMEOUT) -> dict:
    """Sobe um app-server em stdio, chama `metodo` e devolve o `result`.

    Sem parâmetros de chamada: os dois métodos que este caminho usa (`model/list` e
    `account/rateLimits/read`) não os têm. Um `params` opcional que ninguém passa seria peso morto.

    `timeout` existe porque os dois chamadores esperam coisas diferentes: o catálogo é uma tela que
    alguém abriu e pode esperar, o poll de cota tem que caber no teto das outras fontes (8s no
    `cotas._HTTP_TIMEOUT`) — todas as leituras são aguardadas juntas ali, então a mais lenta é quem
    manda na resposta do `/api/cotas`.

    NÃO dá pra usar `subprocess.run(input=...)`: medido em 30/08/2026, com o stdin fechado junto
    com a entrada o app-server responde o `initialize` e SAI (rc=0, 0,25s) sem chegar no segundo
    pedido — a resposta voltava vazia com sucesso aparente. O canal fica aberto até ela chegar.

    `encoding` explícito pelo mesmo motivo dos outros: `text=True` sozinho decodifica pelo locale
    (cp1252 no Windows), e os rótulos de modelo não são só ASCII.
    """
    proc = subprocess.Popen(
        [_binario(), "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace", bufsize=1,
    )
    # O teto de tempo mata o processo em vez de embrulhar o `readline`: um app-server que trava sem
    # fechar o stdout deixaria a leitura pendurada pra sempre, e é o pane de quem usa que paga.
    carrasco = threading.Timer(timeout, proc.kill)
    carrasco.daemon = True
    carrasco.start()
    try:
        proc.stdin.write("\n".join([
            json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": {"clientInfo": CLIENT_INFO, "capabilities": None}}),
            json.dumps({"jsonrpc": "2.0", "id": 2, "method": metodo, "params": {}}),
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
        # stderr aberto penduraria quem chamou justamente no caminho de falha.
        proc.kill()
        raise RuntimeError((proc.stderr.read() or "").strip()[-500:]
                           or f"codex app-server nao respondeu {metodo}")
    finally:
        carrasco.cancel()
        proc.kill()
        proc.wait()

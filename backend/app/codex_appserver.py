"""Perguntar UMA coisa ao Codex sem sessão viva: um app-server efêmero em stdio.

Existe porque duas telas precisam da mesma máquina e nenhuma delas tem pane: o catálogo de modelos
da abertura (`app/codex_models.py`) e a cota por credencial (`app/cotas.py`). O app-server do pane,
quando existe, é do adapter — este aqui sobe, pergunta e morre.

Medido em 30/08/2026 (codex-cli 0.151.0): `codex app-server` **sem** `--listen` fala JSON-RPC por
linha no stdout, aceita `model/list` (0,78s) e `account/rateLimits/read` (1,2s) sem thread aberta,
sem pane e sem sessão. A credencial que ele usa é a do `~/.codex/auth.json` — que é justamente o
que o painel de cotas quer: uma fonte por CREDENCIAL, não por sessão.

As duas leituras tentam antes o backend do ChatGPT por HTTP (`backend_get`), a mesma rota que o
próprio binário chama com o token da conta; o app-server fica de reserva. Ver "Cota e catálogo do
Codex por HTTP" em docs/decisoes/harnesses.md.

Só stdlib, de propósito: o `scripts/hangar-codex-tui` roda no `python3` do sistema e pode um dia
precisar disto.
"""
import base64
import http.client
import json
import os
import shutil
import subprocess
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

from app import codex_contas

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


class CodexIndisponivel(RuntimeError):
    """O transporte ou o processo não pôde responder ao pedido."""


class CodexRecusado(RuntimeError):
    """O app-server respondeu com uma recusa semântica."""


class CodexRespostaInvalida(RuntimeError):
    """O app-server respondeu, mas o formato não serve ao pedido."""


class CodexAusente(CodexIndisponivel):
    """`codex` não está no PATH deste backend — não é falha do comando, é ausência do binário."""


def _binario() -> str:
    """Caminho do `codex`, resolvido — nunca o nome cru no argv. Mesmo motivo do pi_catalog: no
    Windows o CreateProcess só completa `.exe`, e o `which` aplica o PATHEXT."""
    exe = shutil.which("codex")
    if exe is None:
        raise CodexAusente("nao achei o executavel `codex` no PATH deste servidor — instale o "
                           "Codex CLI ou ajuste o PATH do backend")
    return exe


def _environment(codex_home: str | Path | None) -> dict[str, str]:
    if codex_home is None:
        return dict(os.environ)
    path = Path(codex_home).expanduser().absolute()
    default = codex_contas.default_home().expanduser().absolute()
    account = codex_contas.Account("default", path, True) if path == default else \
        codex_contas.Account("selected", path, False)
    return codex_contas.environment(account, base=os.environ)


def perguntar(metodo: str, timeout: float = _TIMEOUT, *,
              codex_home: str | Path | None = None, params: dict | None = None) -> dict:
    """Sobe um app-server em stdio, chama `metodo` e devolve o `result`.

    `params` fica vazio nas leituras e leva `creditId`/`idempotencyKey` no consumo de uma
    redefinição de cota.

    NÃO dá pra usar `subprocess.run(input=...)`: medido em 30/08/2026, com o stdin fechado junto
    com a entrada o app-server responde o `initialize` e SAI (rc=0, 0,25s) sem chegar no segundo
    pedido — a resposta voltava vazia com sucesso aparente. O canal fica aberto até ela chegar.

    `encoding` explícito pelo mesmo motivo dos outros: `text=True` sozinho decodifica pelo locale
    (cp1252 no Windows), e os rótulos de modelo não são só ASCII.
    """
    try:
        proc = subprocess.Popen(
            # Sem plugins: na largada o app-server confere cada marketplace git num temporário em
            # CODEX_HOME/.tmp, e morto antes de acabar (sempre, aqui) deixa o temporário para trás.
            [_binario(), "app-server", "-c", "features.plugins=false"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace", bufsize=1,
            env=_environment(codex_home),
        )
    except OSError as exc:
        raise CodexIndisponivel(f"codex app-server não iniciou: {exc}") from exc
    # O teto de tempo mata o processo em vez de embrulhar o `readline`: um app-server que trava sem
    # fechar o stdout deixaria a leitura pendurada pra sempre, e é o pane de quem usa que paga.
    carrasco = threading.Timer(timeout, proc.kill)
    carrasco.daemon = True
    carrasco.start()
    try:
        proc.stdin.write("\n".join([
            json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": {"clientInfo": CLIENT_INFO, "capabilities": None}}),
            json.dumps({"jsonrpc": "2.0", "id": 2, "method": metodo, "params": params or {}}),
        ]) + "\n")
        proc.stdin.flush()
        for linha in proc.stdout:
            try:
                msg = json.loads(linha)
            except ValueError:
                continue  # o app-server também escreve notificação e log; linha torta não é erro
            if isinstance(msg, dict) and msg.get("id") == 2:
                if isinstance(msg.get("result"), dict):
                    return msg["result"]
                # Resposta de ERRO e resposta: sem este ramo ela nao casava, o laco seguia ate o EOF
                # e quem chamou ouvia "nao respondeu" — para um servidor que respondeu, dizendo o
                # motivo. Como isto alimenta a cota e o catalogo do Codex, o motivo real sumia do log.
                if "error" in msg:
                    proc.kill()
                    raise CodexRecusado(f"codex app-server recusou {metodo}: {msg['error']}")
                raise CodexRespostaInvalida(
                    f"codex app-server devolveu resposta inválida para {metodo}")
        # Mata ANTES de ler o stderr: o `read()` vai até o EOF, e um processo ainda vivo com o
        # stderr aberto penduraria quem chamou justamente no caminho de falha.
        proc.kill()
        detalhe = (proc.stderr.read() or "").strip()[-500:]
        raise CodexIndisponivel(detalhe or f"codex app-server nao respondeu {metodo}")
    except OSError as exc:
        raise CodexIndisponivel(f"codex app-server perdeu o transporte: {exc}") from exc
    finally:
        carrasco.cancel()
        proc.kill()
        proc.wait()


_BACKEND = "https://chatgpt.com/backend-api"
# Margem pro relógio: token que vence durante a ida vira 401 e custa as duas rotas.
_FOLGA_TOKEN_S = 60.0


def _expira_em(token: str) -> float | None:
    try:
        parte = token.split(".")[1]
        exp = json.loads(base64.urlsafe_b64decode(parte + "=" * (-len(parte) % 4))).get("exp")
    except (IndexError, ValueError, AttributeError):
        return None
    return float(exp) if isinstance(exp, (int, float)) and not isinstance(exp, bool) else None


def backend_get(caminho: str, *, codex_home: str | Path | None = None,
                timeout: float = _TIMEOUT) -> tuple[int, object]:
    """GET no backend do ChatGPT com o token da conta, com os cabeçalhos que o CLI manda.

    Devolve (status, json) quando houve resposta HTTP; json é None fora do 200. Levanta
    `CodexIndisponivel` quando nem dá pra tentar ou não houve resposta — o chamador cai no
    app-server. Nunca renova o token: o refresh é do CLI, e girá-lo aqui deslogaria o CLI.
    """
    raiz = (Path(codex_home) if codex_home is not None else codex_contas.default_home()).expanduser()
    try:
        tokens = json.loads((raiz / "auth.json").read_text(encoding="utf-8")).get("tokens")
        token, conta = tokens["access_token"], tokens["account_id"]
    except (OSError, ValueError, AttributeError, TypeError, KeyError) as exc:
        raise CodexIndisponivel("sem token no auth.json") from exc
    if not (isinstance(token, str) and token and isinstance(conta, str) and conta):
        raise CodexIndisponivel("sem token no auth.json")
    exp = _expira_em(token)
    if exp is None or exp - _FOLGA_TOKEN_S <= time.time():
        raise CodexIndisponivel("token vencido ou ilegivel")
    req = urllib.request.Request(_BACKEND + caminho, method="GET", headers={
        "Authorization": f"Bearer {token}", "ChatGPT-Account-Id": conta, "User-Agent": "codex-cli",
    })
    try:
        with _opener.open(req, timeout=timeout) as r:
            return r.status, json.loads(r.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            raise CodexIndisponivel(f"redirecionado: http {exc.code}") from exc
        return exc.code, None
    except (urllib.error.URLError, OSError, ValueError, TimeoutError,
            http.client.HTTPException) as exc:
        raise CodexIndisponivel(f"sem resposta: {type(exc).__name__}") from exc


class _SemRedirect(urllib.request.HTTPRedirectHandler):
    # Seguir o redirect levaria o token e o id da conta para outro host.
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


_opener = urllib.request.build_opener(_SemRedirect)


def versao() -> str:
    """Versão do `codex` que o app-server usaria. O catálogo HTTP depende dela: a mesma conta com
    `client_version` antigo recebe outra lista."""
    try:
        r = subprocess.run([_binario(), "--version"], capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=10)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise CodexIndisponivel(f"codex --version falhou: {exc}") from exc
    partes = (r.stdout or "").split()
    if r.returncode != 0 or len(partes) < 2 or not partes[-1][:1].isdigit():
        raise CodexIndisponivel("codex --version sem versao")
    return partes[-1]

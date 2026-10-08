"""Codex sem terminal: `codex app-server --stdio` atrás de um cano, sem pane tmux.

O cano é o mesmo do Claude sem terminal (`claude_headless/cano.py`): dono do processo, fora do
cgroup do backend, socket local; o backend conecta, cai e reconecta. O que muda em relação à
sessão com TUI: quem sobe o servidor e abre a thread é o backend, e as aprovações chegam aqui
(na TUI o servidor nunca perguntava: `approval_policy=never` + `danger-full-access`).
"""
from __future__ import annotations

import json
import os
import shutil
import uuid
from pathlib import Path

from app import codex_contas, runtime_config
from app.adapters.claude_headless import adapter as hl_adapter
from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex.appserver import AppServerClient
from app.adapters.codex.lancador import CLIENT_INFO, PERMISSION_POLICIES, service_tier_override, tool_output_override


class Ocupada(RuntimeError):
    """A sessão está num turno; a operação pedida derrubaria a conexão no meio dele."""


class ShutdownPending(RuntimeError):
    """O processo sinalizado ainda está vivo; não é seguro abrir outro escritor."""

# Nomes iguais aos do picker `/permissions` da TUI, pra pílula do app ser a mesma nos dois modos.
# `untrusted` deixou de existir (codex-cli 0.154: o app-server sai com "no longer supported"), então
# o que separa os dois primeiros é só o sandbox — e sandbox não troca ao vivo por RPC
# (`codex_permissions.py`), por isso ele vai no `-c` da subida e trocar é reiniciar o servidor.
MODOS: list[tuple[str, str, str, str]] = [
    ("Ask for approval", *PERMISSION_POLICIES["Ask for approval"],
     "Codex só lê o workspace; editar ou rodar comando pede aprovação."),
    ("Approve for me", *PERMISSION_POLICIES["Approve for me"],
     "Codex edita o workspace sozinho; fora dele ou com rede, pede aprovação."),
    ("Full Access", *PERMISSION_POLICIES["Full Access"],
     "Codex faz tudo sem perguntar."),
]
MODO_PADRAO = "Full Access"


# Pedidos do servidor que viram o cartão Permitir/Negar do app. Os demais com `id`
# (`item/permissions/requestApproval`, `mcpServer/elicitation/request`, `item/tool/call`…) não têm
# tela aqui e recebem -32601, nunca um sucesso vazio calado.
APROVACOES = ("item/commandExecution/requestApproval", "item/fileChange/requestApproval")
OPCOES_APROVACAO = ["Permitir", "Negar", "Sempre permitir"]


def texto_da_aprovacao(req: dict) -> str:
    p = req.get("params") or {}
    if req.get("method") == "item/fileChange/requestApproval":
        alvo = "Editar arquivos"
        if p.get("grantRoot"):
            alvo += f" em {p['grantRoot']}"
    else:
        alvo = f"Rodar `{p.get('command') or '?'}`"
        if p.get("cwd"):
            alvo += f" em {p['cwd']}"
    motivo = p.get("reason")
    return f"{alvo}?" + (f" {motivo}" if motivo else "")


def decisao(option: int) -> str:
    """1 = permitir, 2 = negar, 3 = permitir e não perguntar de novo nesta sessão."""
    return {1: "accept", 3: "acceptForSession"}.get(option, "decline")


def politica(modo: str | None) -> tuple[str, str]:
    """(approval_policy, sandbox_mode) do modo do app; modo desconhecido cai no padrão."""
    for nome, approval, sandbox, _ in MODOS:
        if nome.lower() == (modo or MODO_PADRAO).strip().lower():
            return approval, sandbox
    return politica(MODO_PADRAO)


def modos_para_tela(atual: str | None) -> dict:
    """Mesmo shape do `list_codex_permissions` (picker da TUI), pro front não saber a diferença."""
    atual = atual or MODO_PADRAO
    return {"modes": [{"numero": i + 1, "nome": nome, "desc": desc,
                       "cursor": nome == atual, "atual": nome == atual}
                      for i, (nome, _, _, desc) in enumerate(MODOS)],
            "current": atual}


def argv(meta: dict) -> list[str]:
    approval, sandbox = politica(meta.get("permission_mode"))
    return ["codex", "app-server", "--stdio",
            "-c", f'sandbox_mode="{sandbox}"', "-c", f'approval_policy="{approval}"',
            *tool_output_override(meta.get("tool_output_token_limit")),
            *service_tier_override(meta.get("service_tier"))]


def _ambiente(meta: dict) -> dict:
    env = dict(os.environ)
    # Resolvida pelo id, com a validação da conta: secundária apagada depois da criação falha aqui
    # com nome, não lá na frente com um CODEX_HOME que não existe.
    try:
        account = codex_contas.resolve_account(meta.get("codex_account") or "default")
    except codex_contas.AccountError as exc:
        raise RuntimeError(f"conta Codex indisponível: {exc.code} {exc.params}") from None
    env = codex_contas.environment(account, base=env)
    # Backend subido de dentro de um tmux (dev) passaria o pane do OPERADOR pro processo.
    env.pop("TMUX", None)
    env.pop("TMUX_PANE", None)
    env["CP_SESSION_NAME"] = meta["name"]
    env["CP_SESSION_KEY"] = meta["key"]
    env[hl_adapter._MARCADOR_CANO] = meta["key"]
    env[hl_adapter._CANO_OWNER] = str(Path.home())
    # Escolha da abertura, como no Claude sem terminal: marcador sempre, chave só com o recurso
    # ligado. Herdar do backend daria o Jev a toda sessão.
    env.update(runtime_config.env_jev(bool(meta.get("jev"))))
    return env


async def subir(meta: dict, tarefas: set | None = None, *, prepared: bool = False) -> dict:
    from app.account_lifecycle import complete_on_cancel
    return await complete_on_cancel(_launch_account_cano_owned(meta, tarefas, prepared=prepared))


async def _launch_account_cano_owned(meta: dict, tarefas: set | None = None, *, prepared: bool = False) -> dict:
    """Sobe o cano com o app-server dentro e grava `cano` no sidecar. Devolve o dict do cano."""
    if shutil.which("codex") is None:
        raise RuntimeError("binário não encontrado: codex")
    log = codex_sessions._dir() / f"cano-{meta['key'][:16]}.log"
    comando = argv(meta)
    if prepared:
        codex_sessions.update_transfer_runtime(meta["name"], meta["transfer_id"], launching=True)
    cano, _ = await hl_adapter.subir_cano_processo(comando, cwd=meta["cwd"], env=_ambiente(meta),
                                                   key=meta["key"], log=log, tarefas=tarefas, account_provider="codex")
    if prepared:
        from app.conversation_transfer import _processes
        codex_sessions.update_transfer_runtime(meta["name"], meta["transfer_id"], cano=cano,
                                                processes=_processes(cano["pid"]), launching=False)
    else:
        codex_sessions.update(meta["name"], cano=cano)
    return cano


async def conectar(cano: dict, *, esperar: float = 0.0) -> tuple[AppServerClient, dict] | None:
    """Liga um AppServerClient ao cano e lê o snapshot. None = nada escutando ali."""
    ligacao = await hl_adapter.conectar_cano(cano, esperar=esperar)
    if ligacao is None:
        return None
    lig, snap = ligacao
    cano["versao"] = snap.get("versao", 1)
    client = AppServerClient()
    client.cano_snapshot = snap
    for linha in snap.get("stderr_tail") or []:
        client.stderr_tail.append(linha)
    # Pedidos que o servidor fez enquanto o backend estava fora: voltam pela fila como se
    # estivessem chegando agora, e o adapter monta o cartão de aprovação.
    for bruto in snap.get("pendentes") or []:
        try:
            msg = json.loads(bruto)
        except ValueError:
            continue
        if isinstance(msg, dict) and msg.get("id") is not None and "method" in msg:
            client.server_requests[msg["id"]] = msg
            client._notifications.put_nowait(msg)
    for thread_id, prefix in ((snap.get("inflight") or {}).get("codex") or {}).items():
        if prefix.get("complete") and prefix.get("text"):
            client._notifications.put_nowait({"method": "turn/started", "params": {
                "threadId": thread_id, "turn": {"id": prefix.get("turnId")}}})
            client._notifications.put_nowait({"method": "item/agentMessage/delta", "params": {
                "threadId": thread_id, "turnId": prefix.get("turnId"),
                "itemId": prefix.get("itemId"), "delta": prefix["text"]}})
    # Snapshot antes do leitor: notification nova nunca é substituída por uma pendência velha.
    client._attach(lig.stdout, lig.stdin)
    return client, snap


async def initialize(client: AppServerClient) -> None:
    """`initialize` no app-server; religando no mesmo processo ele responde "Already initialized"
    (-32600), que aqui é sucesso."""
    try:
        await client.request("initialize", {"clientInfo": CLIENT_INFO,
                                            "capabilities": {"experimentalApi": True}})
    except RuntimeError as exc:
        if "already initialized" not in str(exc).lower():
            raise


def rollout_de(thread_id: str, codex_home: str | None) -> str:
    raiz = Path(codex_home or os.environ.get("CODEX_HOME") or (Path.home() / ".codex")).expanduser() / "sessions"
    achados = sorted(raiz.glob(f"*/*/*/rollout-*-{thread_id}.jsonl"),
                     key=lambda p: p.stat().st_mtime, reverse=True)
    return str(achados[0]) if achados else ""


def matar(meta: dict | None) -> None:
    """SIGTERM no grupo do cano (cano + app-server). Idempotente; sem cano não faz nada."""
    cano = (meta or {}).get("cano") or {}
    if cano.get("pid"):
        hl_adapter._matar_grupo(int(cano["pid"]), (meta or {}).get("name") or "?")
    key = (meta or {}).get("key")
    if key:
        for arq in codex_sessions._dir().glob(f"cano-{key[:16]}*"):
            try:
                arq.unlink()
            except OSError:
                pass


def nova_chave() -> str:
    return uuid.uuid4().hex

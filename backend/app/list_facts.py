"""Fatos da lista que continuam do Python (`docs/migracao-rust/lista-estado/desenho.md`, "Fatos por
pergunta e resposta"). O Rust descobre e classifica as linhas Claude; a cada produção da lista ele
manda aqui as linhas e a contagem de clientes do dono, e recebe o que ainda é do Python: estado dos
provedores não migrados, conta de Kimi/Pi/omp, transferências, orquestrações, acesso, navegador,
terminais de atalho e problemas do runtime. Falha levanta: o Rust guarda o último valor e marca as
linhas, nunca troca a lista pela do Python."""
import asyncio
import logging

from app import guest_users, plugin_bridge, registry, runtime_config, share_store, sse
from app.models import SessionInfo

_log = logging.getLogger("hangar.list")

_OTHERS = frozenset({"codex", "pi", "omp", "kimi"})
_STATE_FIELDS = ("state", "label", "question", "options", "problema", "status_line", "pending_questions",
                 "startup_steps", "last_activity", "limited", "limit_reset", "stalled", "codex_service_tier")
# A lista não espera terminal de atalho: o leitor do refresher, com a última leitura boa.
_shortcuts = sse._ListRefresher()
# Campos do `sse._list_sig`, pelo nome: a sombra do Rust compara campo a campo e grava só o nome.
SIG_FIELDS = ("name", "cwd", "branch", "git_cwd", "worktree_gone", "git_dirty",
              "git_ahead", "git_behind", "git_added", "git_removed", "state", "tracked", "headless",
              "jsonl", "question", "stalled", "limited", "lifecycle_id", "transfer_id", "transfer_phase",
              "last_reply", "last_reply_at", "pending_questions", "limit_reset", "then_target", "status_line",
              "context", "model", "label", "startup_steps", "loop_status", "loop_iter", "engine", "conta",
              "codex_service_tier", "plan_name", "plan_done", "plan_total", "plan_task", "plan_task_total",
              "plan_complete", "plan_tasks", "plan_hidden", "problema", "provider", "shared", "owner",
              "orq_arbiter", "pair_peers", "pair_gid", "pair_task", "pair_external")
# Lista servida mais velha que isto não se compara com a produção de agora.
_SHADOW_MAX_AGE = 3.0


async def compute(rows: list[dict], owner_clients: int, pane_pids: dict[str, int], shadow: bool = False) -> dict:
    infos = [SessionInfo.model_validate(r) for r in rows]
    if shadow:
        return await _shadow(infos, pane_pids)
    plugin_bridge.app_remoto(owner_clients)
    others = [i for i in infos if i.provider in _OTHERS]
    if others:
        # O código de hoje, só sobre as linhas que o Rust não classifica.
        await sse._list_registry.list_with_state(others, state_only=True)
    # Tudo o que lê disco numa ida só ao threadpool.
    out = await asyncio.to_thread(_files, infos, others, pane_pids)
    # No laço: `nav_pendente`/`nav_confirmar` mexem no mesmo mapa daqui, sem trava.
    out["nav"] = dict(sse.nav_vivos())
    loop = asyncio.get_running_loop()
    if _shortcuts._loop is not loop:
        # Leitura de outro laço nunca termina neste: recomeça (testes, reinício do servidor).
        _shortcuts._loop, _shortcuts._sc_task, _shortcuts._sc_failing = loop, None, False
    _shortcuts.shortcuts_data = _shortcuts._harvest_shortcuts()
    _shortcuts._launch_shortcuts()
    out["shortcuts"] = _shortcuts.shortcuts_data
    out["shadow"] = None
    return out


async def _shadow(infos: list[SessionInfo], pane_pids: dict[str, int]) -> dict:
    """Rodada em sombra: o Python segue dono da lista. O estado das linhas não migradas sai da lista
    que ele serviu (reclassificar mexeria nos caches dele e capturaria panes de novo), a presença do
    app não é tocada e nem o navegador nem o atalho são lidos. Junto vai a assinatura de cada linha servida."""
    served = sse.recent_list(_SHADOW_MAX_AGE)
    by_name = {i.name: i for i in served or ()}
    others = [by_name[i.name] for i in infos if i.provider in _OTHERS and i.name in by_name]
    out = await asyncio.to_thread(_files, infos, [], pane_pids)
    out["states"] = {i.name: {f: getattr(i, f) for f in (*_STATE_FIELDS, "conta")} for i in others}
    # `nav_vivos` grava ao vencer marcador; a sombra não serve navegador.
    out["nav"] = {}
    out["shortcuts"] = None
    out["shadow"] = None if served is None else {i.name: _row_sig(i) for i in served}
    return out


def _row_sig(i: SessionInfo) -> dict:
    """A tupla do `sse._list_sig`, com nome por campo."""
    d = i.model_dump(mode="json", include=set(SIG_FIELDS))
    d["status_line"] = sse._status_sig(i.status_line)
    d["context"] = sse._context_sig(i.context)
    d["label"] = i.label if i.provider == "codex" and not i.tracked else bool(i.label)
    d["plan_tasks"] = d.get("plan_tasks") or []
    return d


def _files(infos: list[SessionInfo], others: list[SessionInfo], pane_pids: dict[str, int]) -> dict:
    accounts = _accounts(others, pane_pids)
    states = {}
    for info in others:
        states[info.name] = {f: getattr(info, f) for f in _STATE_FIELDS}
        if info.name in accounts:
            states[info.name]["conta"] = accounts[info.name]
    overrides, frozen = _transfers(infos)
    orq, orq_error = _orq_rows()
    names = {i.name for i in infos} | {r["name"] for r in overrides} | {r["name"] for r in orq}
    shared, owners, hidden = _access(names)
    return {
        "states": states,
        "overrides": overrides,
        "frozen": frozen,
        "orq": orq,
        "orq_error": orq_error,
        "shared": shared,
        "owners": owners,
        "hidden": hidden,
        "problems": _problems(infos),
        "held": _held(infos),
        "stall_seconds": float(runtime_config.get("stall_seconds")),
    }


def _accounts(others: list[SessionInfo], pane_pids: dict[str, int]) -> dict[str, str | None]:
    """`registry.list`: Kimi gasta o provider padrão do config; Pi e omp, a credencial do modelo em
    uso, lida no sidecar do catálogo que mora na conta do pane."""
    from app import cotas, pi_models
    out = {}
    for info in others:
        if info.provider == "kimi":
            padrao = cotas.provider_padrao_kimi()
            out[info.name] = f"kimi:{padrao}" if padrao else None
        elif info.provider in ("pi", "omp"):
            pid = pane_pids.get(info.name)
            cfg = registry._config_dir_of(pid) if pid else None
            atual = pi_models.provider_atual(info.jsonl, cfg) if info.jsonl else None
            out[info.name] = cotas.conta_de_provider_pi(atual)
    return out


def _transfers(infos: list[SessionInfo]) -> tuple[list[dict], list[str]]:
    """Linhas que a transferência substitui ou cria, inteiras, e as em curso, que o Rust não
    classifica nem decora (`list_with_state` as separa do mesmo jeito)."""
    from app.conversation_transfer import TransferPhase
    terminal = {p.value for p in (TransferPhase.COMPLETE, TransferPhase.REJECTED, TransferPhase.ROLLED_BACK)}
    rows = [i.model_copy() for i in infos]
    registry._decorate_transfers(rows)
    touched = [i for i in rows if i.transfer_id]
    return ([i.model_dump(mode="json") for i in touched],
            [i.name for i in touched if i.transfer_phase and i.transfer_phase not in terminal])


def _orq_rows() -> tuple[list[dict], str | None]:
    """Linhas `orq` e o código da falha: com ele, o Rust fica com as da última resposta boa, marcadas,
    em vez de servir a lista sem as orquestrações calado."""
    try:
        runs = registry.orq_runs.active()
    except Exception:
        _log.warning("orq: leitura das orquestrações falhou (vale a última)", exc_info=True)
        return [], "orq_unreadable"
    out = []
    for run in runs:
        state, last = registry.orq_runs.activity(run["timeline"])
        out.append(SessionInfo(name=run["name"], cwd=run["repo"], jsonl=run["timeline"], provider="orq",
                               tracked=True, pair_gid=run["gid"], orq_arbiter=run["arbiter"],
                               state=state, last_activity=last).model_dump(mode="json"))
    return out, None


def _access(names: set[str]) -> tuple[list[str], dict[str, str], list[str]]:
    shared = sorted(share_store.active_sessions() & names)
    if not guest_users.has_claims():
        return shared, {}, []
    owners = {n: o for n in names if (o := guest_users.owner_name(n))}
    hidden = sorted(n for n in names if not guest_users.visible_to(None, n))
    return shared, owners, hidden


def _held(infos: list[SessionInfo]) -> dict[str, dict]:
    """Pergunta que o hook do plugin segura agora, das linhas Claude com terminal. A permissão
    segurada para o app não desenha cartão no pane e o registro nativo segue `busy`: sem isto a
    lista diria `working`."""
    out = {}
    for info in infos:
        if info.provider == "claude" and not info.headless and (q := plugin_bridge.pergunta_pendente(info.name)):
            out[info.name] = q
    return out


def _problems(infos: list[SessionInfo]) -> dict[str, str]:
    """Problema do runtime das linhas Claude: o registrado pelo adaptador sem terminal, ou o que o
    Rust publicou no vínculo da sessão com terminal."""
    from app.adapters import CLAUDE_HEADLESS, get_adapter
    from app.runtime_adapter import runtime_problem
    hl = get_adapter(CLAUDE_HEADLESS)
    out = {}
    for info in infos:
        if info.provider != "claude":
            continue
        try:
            problem = hl.problema_de(info.name) if info.headless else runtime_problem(info.name)
        except RuntimeError:
            # Sessão cujo runtime ainda não publicou retrato: sem fato novo só para ela; derrubar os fatos
            # de todas tirava a lista inteira do ar.
            continue
        if problem:
            out[info.name] = problem[0]
    return out

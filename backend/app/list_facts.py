"""Fatos da lista que continuam do Python (`docs/migracao-rust/lista-estado/desenho.md`, "Fatos por
pergunta e resposta"). O Rust descobre, classifica e lê do disco tudo o que é de Claude e Codex; só
com linha Pi, omp ou Kimi na lista ele manda essas linhas aqui e recebe o estado delas.
Falha levanta: o Rust guarda o último valor e marca as linhas, nunca troca a lista pela do Python."""
from app import sse
from app.internal_pool import internal_thread
from app.models import SessionInfo

_OTHERS = frozenset({"pi", "omp", "kimi"})
_STATE_FIELDS = ("state", "label", "question", "options", "problema", "status_line", "pending_questions",
                 "startup_steps", "last_activity", "limited", "limit_reset", "stalled",
                 "last_reply", "last_reply_at")


async def compute(rows: list[dict], pane_pids: dict[str, int]) -> dict:
    infos = [SessionInfo.model_validate(r) for r in rows]
    others = [i for i in infos if i.provider in _OTHERS]
    if others:
        # O código de hoje, só sobre as linhas que o Rust não classifica.
        await sse._list_registry.list_with_state(others, state_only=True)
    # Tudo o que lê disco numa ida só ao threadpool.
    return await internal_thread(_files, others, pane_pids)


def _files(others: list[SessionInfo], pane_pids: dict[str, int]) -> dict:
    return {"states": {info.name: {field: getattr(info, field) for field in _STATE_FIELDS} for info in others}}

"""Estado ao vivo da sessão Codex (ticket 02).

Ligar o marcador de estado do Codex faz o backend passar a rodar, também nesse provider, o gatilho
de transição — e lá dentro há caminhos que digitam no pane. Estes casos travam a regressão: no
Codex a entrega é a do adapter (`turn/start` no app-server), e tecla no pane poria a mensagem do
usuário duas vezes na conversa.
"""
import asyncio
import json
from pathlib import Path

import pytest

from app.adapters.codex.adapter import status_line_do_rollout

from app import api, registry
from app.models import SessionInfo, session_key
from app.state import codex_turno_aberto

# O id que o hook entrega, e que `session_key` tem que extrair do nome do rollout.
_ID = "01a05077-a46d-7cb3-a0cd-9d850d4baec4"


async def test_drenar_no_codex_usa_o_adapter_e_nao_o_teclado(monkeypatch):
    monkeypatch.setattr(api, "_loop_servidor", asyncio.get_running_loop())
    chamou = []

    class _Adapter:
        async def drain(self, name: str, path: str) -> int:
            chamou.append((name, path))
            return 3

    monkeypatch.setattr(api, "get_adapter", lambda provider: _Adapter())
    monkeypatch.setattr(api, "drain", lambda *a, **kw: pytest.fail("digitou no pane do Codex"))

    # `to_thread` porque é assim que os chamadores reais rodam (Timer da confirmação, gatilho do
    # hook): a ponte tem que funcionar de FORA do loop.
    assert await asyncio.to_thread(api._drenar, "cx", "/r.jsonl", "codex") == 3
    assert chamou == [("cx", "/r.jsonl")]


async def test_drenar_nos_outros_providers_segue_pelo_teclado(monkeypatch):
    monkeypatch.setattr(api, "_loop_servidor", asyncio.get_running_loop())
    monkeypatch.setattr(api, "get_adapter",
                        lambda provider: pytest.fail("desviou o Claude pro adapter"))
    monkeypatch.setattr(api, "drain", lambda name, jsonl, provider: 2)
    assert await asyncio.to_thread(api._drenar, "cc", "/t.jsonl", "claude") == 2


async def test_drenar_no_codex_sem_loop_deixa_a_fila_pendente(monkeypatch):
    # Falha VISÍVEL (a bolha "na fila" continua na tela), nunca mensagem duplicada.
    monkeypatch.setattr(api, "_loop_servidor", None)
    monkeypatch.setattr(api, "drain", lambda *a, **kw: pytest.fail("digitou no pane do Codex"))
    assert api._drenar("cx", "/r.jsonl", "codex") == 0


# -- O turno preso -------------------------------------------------------------

async def test_marcador_ocioso_destrava_turno_preso(tmp_path, monkeypatch):
    """`in_progress` só é limpo pelo `turn/completed`, e ele só chega a quem assinou a thread. Se
    ele se perde (o app-server foi retomado no meio, o SSE caiu), a sessão fica "ocupada" PARA
    SEMPRE: todo envio vira fila e a fila nunca drena — medido em 30/08/2026, com a lista mostrando
    a sessão ociosa e o `/input` respondendo "sessão ocupada" ao mesmo tempo. O escape por tempo
    que existia só valia para sessão NÃO assinada.

    O marcador do hook é a fonte independente: ele diz que o turno fechou, e quem sabe disso é o
    Codex, não o nosso estado em memória."""
    from app.adapters.codex.adapter import CodexAdapter

    rollout = _rollout(tmp_path, _ev("task_started"), _ev("task_complete"))
    ad = CodexAdapter()
    ad._sessions["cx"] = {"in_progress": True, "subscribed": True, "in_progress_since": 0.0}
    monkeypatch.setattr("app.adapters.codex.adapter.codex_sessions.load",
                        lambda name: {"rollout_path": rollout})
    monkeypatch.setattr("app.adapters.codex.adapter.hook_state.get_state",
                        lambda sid: ("idle", 1.0) if sid == _ID else None)

    assert await ad.deliverable("cx") is True
    assert ad._sessions["cx"]["in_progress"] is False


async def test_marcador_trabalhando_nao_destrava(tmp_path, monkeypatch):
    # O marcador só pode DESTRAVAR. Turno andando de verdade continua segurando o envio, senão o
    # texto entra no meio do turno em curso.
    from app.adapters.codex.adapter import CodexAdapter

    rollout = _rollout(tmp_path, _ev("task_started"))
    ad = CodexAdapter()
    ad._sessions["cx"] = {"in_progress": True, "subscribed": True, "in_progress_since": 0.0}
    monkeypatch.setattr("app.adapters.codex.adapter.codex_sessions.load",
                        lambda name: {"rollout_path": rollout})
    monkeypatch.setattr("app.adapters.codex.adapter.hook_state.get_state",
                        lambda sid: ("working", 1.0))

    assert await ad.deliverable("cx") is False


# -- A chave de estado ---------------------------------------------------------

def test_session_key_do_rollout_e_o_id_que_o_hook_entrega():
    # O rollout se chama rollout-<data>T<hora>-<id>.jsonl e o `session_meta` da primeira linha traz
    # exatamente esse <id> (conferido nos rollouts reais desta máquina) — que é também o
    # `session_id` do payload do hook. Sem isto a chave seria o nome inteiro do arquivo e o
    # marcador gravado pelo hook nunca seria encontrado: sessão eternamente ociosa.
    assert session_key(
        "/home/x/.codex/sessions/2026/08/29/"
        "rollout-2026-08-29T23-20-13-01a05077-a46d-7cb3-a0cd-9d850d4baec4.jsonl"
    ) == "01a05077-a46d-7cb3-a0cd-9d850d4baec4"


def _rollout(tmp_path, *linhas: dict):
    p = tmp_path / "rollout-2026-08-30T10-00-00-01a05077-a46d-7cb3-a0cd-9d850d4baec4.jsonl"
    p.write_text("".join(json.dumps(o) + "\n" for o in linhas), encoding="utf-8")
    return str(p)


def _ev(tipo: str, **extra):
    return {"timestamp": "2026-08-30T10:00:00.000Z", "type": "event_msg",
            "payload": {"type": tipo, **extra}}


def test_turno_aberto_no_rollout(tmp_path):
    # É o que separa "sessão parada" de "trabalhando sem hook aprovado".
    assert codex_turno_aberto(_rollout(tmp_path, _ev("task_started"))) is True
    assert codex_turno_aberto(_rollout(tmp_path, _ev("task_started"), _ev("task_complete"))) is False
    assert codex_turno_aberto(_rollout(tmp_path, _ev("task_started"), _ev("turn_aborted"))) is False
    # Fronteira decidida pelo `payload.type` via json, não pelo regex: uma mensagem do usuário que
    # CITE "task_complete" não pode fechar turno nenhum.
    citando = {"timestamp": "2026-08-30T10:00:02.000Z", "type": "response_item", "payload": {
        "type": "message", "role": "user",
        "content": [{"type": "input_text", "text": 'o evento "type":"task_complete" existe?'}]}}
    assert codex_turno_aberto(_rollout(tmp_path, _ev("task_started"), citando)) is True


def test_turno_aberto_sem_fronteira_nenhuma_e_desconhecido(tmp_path):
    # None = não deu pra saber. Nunca False: "não sei" não pode virar "está parada".
    assert codex_turno_aberto(_rollout(tmp_path, {"type": "session_meta", "payload": {}})) is None
    assert codex_turno_aberto("/nao/existe.jsonl") is None


async def test_a_lista_le_o_marcador_do_codex_sem_raspar_o_pane(tmp_path, monkeypatch):
    # Até aqui toda sessão Codex saía da lista como "ociosa" por construção. O pane continua
    # PROIBIDO: a TUI do Codex não tem régua nem caixa de composer, e `classify` devolveria as duas
    # últimas linhas verbatim — uma segunda statusline, pior que a que o adapter monta.
    rollout = _rollout(tmp_path, _ev("task_started"))
    reg = registry.SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(reg, "list", lambda: [
        SessionInfo(name="cx", cwd="/p", jsonl=rollout, tracked=True, provider="codex")])
    monkeypatch.setattr(registry.hook_state, "get_state",
                        lambda sid: ("working", 1.0) if sid == _ID else None)
    monkeypatch.setattr(registry.tmux, "capture_pane",
                        lambda name, lines=200: pytest.fail("raspou o pane de uma sessão Codex"))

    out = {s.name: s for s in await reg.list_with_state()}
    assert out["cx"].state == "working"
    assert out["cx"].problema is None


async def test_codex_trabalhando_sem_marcador_vira_problema_explicito(tmp_path, monkeypatch):
    # O único modo de falha do estado por hook: hook não aprovado na TUI não roda e não avisa.
    # Calado, isso é uma sessão eternamente ociosa enquanto trabalha.
    rollout = _rollout(tmp_path, _ev("task_started"))
    reg = registry.SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(reg, "list", lambda: [
        SessionInfo(name="cx", cwd="/p", jsonl=rollout, tracked=True, provider="codex")])
    monkeypatch.setattr(registry.hook_state, "get_state", lambda sid: None)
    monkeypatch.setattr(registry.tmux, "capture_pane",
                        lambda name, lines=200: pytest.fail("raspou o pane de uma sessão Codex"))

    out = {s.name: s for s in await reg.list_with_state()}
    assert out["cx"].problema == "codex_hooks_nao_aprovados"


async def test_codex_parada_e_sem_marcador_nao_acusa_nada(tmp_path, monkeypatch):
    # Sessão que só terminou o turno não tem problema nenhum — acusar aqui seria alarme falso em
    # toda sessão Codex parada.
    rollout = _rollout(tmp_path, _ev("task_started"), _ev("task_complete"))
    reg = registry.SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(reg, "list", lambda: [
        SessionInfo(name="cx", cwd="/p", jsonl=rollout, tracked=True, provider="codex")])
    monkeypatch.setattr(registry.hook_state, "get_state", lambda sid: None)
    monkeypatch.setattr(registry.tmux, "capture_pane", lambda name, lines=200: "")

    out = {s.name: s for s in await reg.list_with_state()}
    assert out["cx"].state == "idle" and out["cx"].problema is None


# -- A statusline do card ------------------------------------------------------

def test_statusline_do_card_sai_do_proprio_rollout():
    # O card não tem SSE aberto (é a lista), e a TUI do Codex não pode ser raspada. A linha sai do
    # MESMO arquivo que o chat já lê: modelo do `turn_context`, contexto e cota do `token_count`.
    # Os nomes dos campos ali são snake_case (o app-server usa camelCase) — é a tradução que este
    # caso trava.
    p = str(Path(__file__).parent / "fixtures" / "codex" / "rollout_sample.jsonl")
    linha = status_line_do_rollout(p)
    assert linha and "🤖 gpt-5.4-mini" in linha
    assert "💬" in linha            # contexto do último turno
    assert "📅7d:" in linha          # a janela semanal que aquele rollout registra


async def test_o_card_da_sessao_codex_ganha_a_statusline(tmp_path, monkeypatch):
    fixture = (Path(__file__).parent / "fixtures" / "codex" / "rollout_sample.jsonl").read_text()
    rollout = tmp_path / "rollout-2026-08-30T10-00-00-01a05077-a46d-7cb3-a0cd-9d850d4baec4.jsonl"
    rollout.write_text(fixture, encoding="utf-8")
    reg = registry.SessionRegistry(projects_dir=tmp_path)
    # Nome próprio: `_status_cache` é atributo de CLASSE (compartilhado entre instâncias e entre
    # testes), e dentro do TTL um "cx" de outro caso devolveria a linha dele.
    monkeypatch.setattr(reg, "list", lambda: [
        SessionInfo(name="cx-status", cwd="/p", jsonl=str(rollout), tracked=True,
                    provider="codex")])
    monkeypatch.setattr(registry.hook_state, "get_state", lambda sid: ("idle", 1.0))
    monkeypatch.setattr(registry.tmux, "capture_pane",
                        lambda name, lines=200: pytest.fail("raspou o pane de uma sessão Codex"))

    out = {s.name: s for s in await reg.list_with_state()}
    assert "🤖 gpt-5.4-mini" in (out["cx-status"].status_line or "")


def test_statusline_de_rollout_sem_nada_util_e_none(tmp_path):
    # None = "não tenho linha", e o chamador preserva a última boa em vez de piscar o card.
    vazio = _rollout(tmp_path, {"type": "session_meta", "payload": {}})
    assert status_line_do_rollout(vazio) is None
    assert status_line_do_rollout("/nao/existe.jsonl") is None


def test_session_key_dos_outros_layouts_nao_muda():
    assert session_key("/home/x/.claude/projects/p/abc-123.jsonl") == "abc-123"
    assert session_key("/home/x/.kimi-code/sessions/w/sid-9/agents/main/wire.jsonl") == "sid-9"
    # Nome que só PARECE rollout (sem o UUID no fim) segue caindo no stem, como sempre.
    assert session_key("/tmp/rollout-de-ontem.jsonl") == "rollout-de-ontem"

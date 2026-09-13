"""Claude sem terminal: a máquina de estados do stdout e o que vai pro stdin, sem processo real.
O processo é trocado por um stub que só guarda o que seria escrito; os eventos são os do
stream-json medido contra a CLI (docs/research/claude-sem-terminal-monocode.md)."""
import asyncio
import json

import pytest

from app.adapters.claude_headless import sessions as S
from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter, _Sessao
from app.adapters.codex.preview import CodexPreviewSource


class _Proc:
    returncode = None
    pid = 4242


@pytest.fixture
def sidecar(tmp_path, monkeypatch):
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(CodexPreviewSource, "_sources", {})
    return S.save("s1", str(tmp_path), "11111111-1111-1111-1111-111111111111", model="haiku", permission_mode="manual")


@pytest.fixture
def adapter(sidecar):
    ad = ClaudeHeadlessAdapter()
    sess = _Sessao("s1", sidecar)
    sess.proc = _Proc()
    ad._sessions["s1"] = sess
    escritos: list[dict] = []

    async def _write(s, obj):
        escritos.append(obj)
    ad._write = _write  # type: ignore[method-assign]
    ad.escritos = escritos  # type: ignore[attr-defined]
    return ad


def _run(coro):
    return asyncio.run(coro)


def test_prompt_vai_pro_stdin_e_turno_fecha_no_result(adapter):
    async def fluxo():
        assert await adapter.send_prompt("s1", "oi") == "sent"
        sess = adapter._sessions["s1"]
        assert sess.state == "working" and not await adapter.deliverable("s1")
        await adapter._on_event(sess, {"type": "system", "subtype": "status", "status": "requesting"})
        assert sess.label == "Pensando…"
        await adapter._on_event(sess, {"type": "stream_event", "event": {"type": "content_block_start", "content_block": {"type": "text"}}})
        await adapter._on_event(sess, {"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "ok"}}})
        assert CodexPreviewSource.get("s1").text == "ok"
        await adapter._on_event(sess, {"type": "assistant", "message": {"content": [{"type": "text", "text": "ok"}]}})
        assert CodexPreviewSource.get("s1").text == ""
        await adapter._on_event(sess, {"type": "result", "subtype": "success", "num_turns": 1, "total_cost_usd": 0.04,
                                       "usage": {"input_tokens": 2, "cache_read_input_tokens": 39000, "output_tokens": 4},
                                       "modelUsage": {"claude-haiku-4-5": {"contextWindow": 200000}}})
        assert sess.state == "idle" and await adapter.deliverable("s1")
        assert adapter.status_line(sess) == "🤖 haiku │ 💬 39k/4 39k/200k │ 💵 $0.04"
    _run(fluxo())
    msg = adapter.escritos[0]
    assert msg["type"] == "user" and msg["message"]["content"] == [{"type": "text", "text": "oi"}]


def test_permissao_vira_awaiting_e_opcao_responde(adapter):
    sess = adapter._sessions["s1"]
    req = {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "curl x"}, "description": "Baixa x"}

    async def fluxo():
        sess.in_progress = True
        await adapter._on_event(sess, {"type": "control_request", "request_id": "r1", "request": req})
        ev = adapter._evento(sess)
        assert ev.state == "awaiting_input" and ev.options == ["Permitir", "Negar"]
        assert ev.question == "Permitir Bash? Baixa x"
        assert await adapter.select("s1", 2) is True
        assert sess.state == "working" and not sess.pending
        assert await adapter.select("s1", 1) is False   # nada pendente
    _run(fluxo())
    resp = adapter.escritos[-1]
    assert resp["type"] == "control_response"
    assert resp["response"]["request_id"] == "r1"
    assert resp["response"]["response"]["behavior"] == "deny"


def test_sempre_permitir_so_com_sugestao_e_leva_as_regras(adapter):
    sess = adapter._sessions["s1"]
    regras = [{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "curl *"}],
               "behavior": "allow", "destination": "localSettings"}]

    async def fluxo():
        sess.in_progress = True
        await adapter._on_event(sess, {"type": "control_request", "request_id": "r1",
                                       "request": {"subtype": "can_use_tool", "tool_name": "Bash",
                                                   "input": {"command": "curl x"}, "permission_suggestions": regras}})
        assert adapter._evento(sess).options == ["Permitir", "Negar", "Sempre permitir"]
        assert await adapter.select("s1", 3) is True
        # Sem sugestão, a 3ª opção não existe e o 3 cai em negar.
        await adapter._on_event(sess, {"type": "control_request", "request_id": "r2",
                                       "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {}}})
        assert adapter._evento(sess).options == ["Permitir", "Negar"]
        await adapter.select("s1", 3)
    _run(fluxo())
    r1, r2 = [e["response"]["response"] for e in adapter.escritos if e["type"] == "control_response"]
    assert r1["behavior"] == "allow" and r1["updatedPermissions"] == regras
    assert r2["behavior"] == "deny"


def test_steer_queue_so_com_turno_em_voo_e_sem_pendencia(adapter, tmp_path, monkeypatch):
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    sess = adapter._sessions["s1"]
    q = pqueue.PromptQueue("s1"); q.clear()
    entrada = q.append("agora", delivered=False)

    async def fluxo():
        with pytest.raises(RuntimeError):
            await adapter.steer_queue("s1")            # ociosa: nada a orientar
        sess.in_progress = True
        sess.pending["r1"] = {"subtype": "can_use_tool", "tool_name": "Bash", "input": {}}
        with pytest.raises(RuntimeError):
            await adapter.steer_queue("s1")            # parada em permissão
        sess.pending.clear()
        assert await adapter.steer_queue("s1") == [entrada["id"]]
    _run(fluxo())
    assert adapter.escritos[-1]["message"]["content"][0]["text"] == "agora"
    assert all(e.get("delivered") for e in q.load())
    q.clear()


def test_ask_user_question_vira_pergunta_nativa_e_resposta_leva_rotulos(adapter):
    sess = adapter._sessions["s1"]
    perguntas = [{"question": "A ou B?", "header": "Escolha", "multiSelect": False,
                  "options": [{"label": "A", "description": ""}, {"label": "B", "description": ""}]}]

    async def fluxo():
        sess.in_progress = True
        await adapter._on_event(sess, {"type": "control_request", "request_id": "q1",
                                       "request": {"subtype": "can_use_tool", "tool_name": "AskUserQuestion",
                                                   "input": {"questions": perguntas}}})
        ev = adapter._evento(sess)
        assert ev.state == "awaiting_input" and ev.options is None
        assert ev.codex_question == {"provider": "claude", "request_id": "q1", "questions": perguntas}
        with pytest.raises(ValueError):
            await adapter.answer_questions("s1", "outra", [{"kind": "option", "indices": [1]}])
        await adapter.answer_questions("s1", "q1", [{"kind": "option", "indices": [1]}])
        assert sess.question is None and sess.state == "working"
    _run(fluxo())
    resp = adapter.escritos[-1]["response"]["response"]
    assert resp["behavior"] == "allow"
    assert resp["updatedInput"] == {"questions": perguntas, "answers": {"A ou B?": "B"}}


def test_clear_troca_o_sid_no_sidecar(adapter, sidecar):
    sess = adapter._sessions["s1"]
    _run(adapter._on_event(sess, {"type": "system", "subtype": "init", "session_id": "22222222-2222-2222-2222-222222222222",
                                  "model": "claude-sonnet-5", "permissionMode": "acceptEdits"}))
    assert S.load("s1")["session_id"] == "22222222-2222-2222-2222-222222222222"
    assert sess.model == "claude-sonnet-5" and sess.permission_mode == "acceptEdits"


def test_interrupt_nega_pendencias_antes(adapter):
    sess = adapter._sessions["s1"]

    async def fluxo():
        sess.in_progress = True
        await adapter._on_event(sess, {"type": "control_request", "request_id": "r1",
                                       "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {}}})
        assert await adapter.interrupt("s1") is True
    _run(fluxo())
    tipos = [(e["type"], e.get("request", {}).get("subtype")) for e in adapter.escritos]
    assert tipos == [("control_response", None), ("control_request", "interrupt")]


def test_sessao_parada_fica_ociosa_sem_subir_processo(sidecar, monkeypatch):
    ad = ClaudeHeadlessAdapter()
    monkeypatch.setattr(ad, "_spawn", lambda sess: (_ for _ in ()).throw(AssertionError("não devia subir")))

    async def fluxo():
        gen = ad.state_monitor("s1", lambda: None)
        ev = await gen.__anext__()
        assert ev.state == "idle" and ev.claude_permission_mode == "manual"
        await gen.aclose()
    _run(fluxo())


def test_registry_cria_lista_e_mata_sem_tmux(tmp_path, monkeypatch):
    from app import registry as R
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(R.tmux, "has_session", lambda n: False)
    monkeypatch.setattr(R.tmux, "list_panes_all", lambda: {})
    monkeypatch.setattr(R, "_pretrust_cwd", lambda cwd, cfg: None)
    reg = R.SessionRegistry(str(tmp_path / "projects"))
    info = reg.create("hl", str(tmp_path), provider="claude", headless=True, model="haiku", permission_mode="manual")
    assert info.headless and info.provider == "claude" and info.jsonl.endswith(".jsonl")
    assert S.load("hl")["cwd"] == str(tmp_path)
    with pytest.raises(ValueError):
        reg.create("hl", str(tmp_path), provider="claude")          # nome ocupado pelo sidecar
    with pytest.raises(ValueError):
        reg.create("x", str(tmp_path), provider="pi", headless=True)  # só claude
    listadas = [i for i in reg.list() if i.name == "hl"]
    assert len(listadas) == 1 and listadas[0].headless and listadas[0].tracked
    reg.kill("hl")
    assert not S.exists("hl")


def test_turno_com_erro_vira_problema_e_sucesso_limpa(adapter):
    sess = adapter._sessions["s1"]

    async def fluxo():
        sess.in_progress = True
        await adapter._on_event(sess, {"type": "result", "subtype": "error_max_turns", "is_error": True,
                                       "result": "Reached max turns", "usage": {}})
        ev = adapter._evento(sess)
        assert ev.state == "idle" and ev.problema == "headless_turno_erro"
        assert "Reached max turns" in (ev.problema_detalhe or "")
        assert adapter.problema_de("s1") == (ev.problema, ev.problema_detalhe)
        # Interrupt não é erro: nada muda.
        sess.in_progress = True
        await adapter._on_event(sess, {"type": "result", "subtype": "error_during_execution", "usage": {}})
        assert adapter._evento(sess).problema == "headless_turno_erro"
        sess.in_progress = True
        await adapter._on_event(sess, {"type": "result", "subtype": "success", "usage": {"input_tokens": 1}})
        assert adapter._evento(sess).problema is None and adapter.problema_de("s1") is None
    _run(fluxo())


def test_processo_caindo_registra_problema_com_stderr(adapter, sidecar):
    sess = adapter._sessions["s1"]
    sess.stderr_tail.append("Error: not logged in")

    class _Fim:
        async def readline(self):
            return b""

    class _Morto:
        returncode = 3
        pid = 4242
        stdout = _Fim()

        async def wait(self):
            return 3

    sess.proc = _Morto()   # type: ignore[assignment]

    async def fluxo():
        await adapter._ler(sess)   # stdout no EOF: é o caminho da morte do processo
        assert sess.state == "dead"
        assert adapter.problema_de("s1")[0] == "headless_processo_caiu"
        assert "not logged in" in adapter.problema_de("s1")[1]
        # Parada, a sessão publica o problema da última vida.
        adapter._sessions.pop("s1")
        gen = adapter.state_monitor("s1", lambda: None)
        ev = await gen.__anext__()
        assert ev.state == "idle" and ev.problema == "headless_processo_caiu"
        await gen.aclose()
    _run(fluxo())


def test_ensure_running_nao_sobe_dois_processos(sidecar, monkeypatch):
    ad = ClaudeHeadlessAdapter()
    subidas = []

    async def spawn_falso(sess):
        subidas.append(sess.sid)
        await asyncio.sleep(0.05)
        sess.proc = _Proc()
    monkeypatch.setattr(ad, "_spawn", spawn_falso)

    async def fluxo():
        a, b = await asyncio.gather(ad.ensure_running("s1"), ad.ensure_running("s1"))
        assert a is b and len(subidas) == 1
    _run(fluxo())


def test_registry_renomeia_sem_tmux(tmp_path, monkeypatch):
    from app import registry as R
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(R.tmux, "has_session", lambda n: False)
    monkeypatch.setattr(R.tmux, "list_panes_all", lambda: {})
    monkeypatch.setattr(R, "_pretrust_cwd", lambda cwd, cfg: None)
    reg = R.SessionRegistry(str(tmp_path / "projects"))
    reg.create("hl", str(tmp_path), provider="claude", headless=True)
    reg.rename("hl", "hl2")
    assert not S.exists("hl") and S.load("hl2")["name"] == "hl2"
    reg.kill("hl2")

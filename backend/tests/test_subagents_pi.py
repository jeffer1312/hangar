"""Subagentes do Pi (tool `subagent`).

Layout: `<transcript-sem-.jsonl>/<taskId>/run-<n>/session.jsonl`. As linhas copiam o shape REAL
(medido numa sessao do usuario em 27/08/2026, Pi 0.82.1) — inventar um shape "que a doc sugere" ja
deixou passar bug de parser antes (ver o `tool.result` sem uuid no CLAUDE.md).
"""

import json

import pytest

from app.subagents import get_subagent, list_subagents


def _linhas(xs: list[dict]) -> str:
    return "\n".join(json.dumps(x, ensure_ascii=False) for x in xs) + "\n"


def _filho(agente: str, run_uuid: str, *, tools: int = 2, texto: str = "pronto",
           parada: str | None = "stop") -> str:
    xs: list[dict] = [
        {"type": "session", "version": 3, "id": "01a04446",
         "timestamp": "2026-08-27T17:31:10.588Z", "cwd": "/repo"},
        {"type": "session_info", "id": "e3d33a56", "parentId": "e6aa4277",
         "timestamp": "2026-08-27T17:31:20.838Z",
         "name": f"subagent-{agente}-{run_uuid}-1"},
        {"type": "message", "id": "m1", "parentId": "e3d33a56",
         "timestamp": "2026-08-27T17:31:21.000Z",
         "message": {"role": "user", "content": [{"type": "text", "text": "Task: confira o MR"}]}},
    ]
    for i in range(tools):
        xs.append({"type": "message", "id": f"a{i}", "parentId": "m1",
                   "timestamp": "2026-08-27T17:31:30.000Z",
                   "message": {"role": "assistant", "stopReason": "toolUse", "content": [
                       {"type": "toolCall", "id": f"call_{i}", "name": "bash",
                        "arguments": {"command": f"git log -{i}"}}]}})
        xs.append({"type": "message", "id": f"r{i}", "parentId": f"a{i}",
                   "timestamp": "2026-08-27T17:31:31.000Z",
                   "message": {"role": "toolResult", "toolCallId": f"call_{i}",
                               "toolName": "bash", "isError": False,
                               "content": [{"type": "text", "text": "ok"}]}})
    fim = {"role": "assistant", "content": [{"type": "text", "text": texto}]}
    if parada is not None:
        fim["stopReason"] = parada
    xs.append({"type": "message", "id": "fim", "parentId": "m1",
               "timestamp": "2026-08-27T17:31:40.000Z", "message": fim})
    return _linhas(xs)


@pytest.fixture
def raiz_pi(tmp_path, monkeypatch):
    """Vira a raiz de sessoes do Pi pro tmp — `sessions_root()` le esta env var."""
    r = tmp_path / "sessions"
    (r / "--repo--").mkdir(parents=True)
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(r))
    return r / "--repo--"


def _sessao(raiz, filhos: dict[str, dict[str, str]]) -> str:
    """Monta o transcript do pai + `<stem>/<taskId>/<run-N>/session.jsonl`, devolve o do pai."""
    stem = "2026-08-27T15-27-07-379Z_8d48a6d5"
    pai = raiz / f"{stem}.jsonl"
    pai.write_text(_linhas([{"type": "session", "version": 3, "id": "x"}]), encoding="utf-8")
    for task, runs in filhos.items():
        for run, conteudo in runs.items():
            d = raiz / stem / task / run
            d.mkdir(parents=True)
            (d / "session.jsonl").write_text(conteudo, encoding="utf-8")
    return str(pai)


def test_lista_traz_tipo_prompt_e_ferramentas(raiz_pi):
    pai = _sessao(raiz_pi, {
        "ee6989ae-1db7-4759-ab8c-e3cc9a975ff4": {
            "run-0": _filho("delegate", "ee6989ae-1db7-4759-ab8c-e3cc9a975ff4", tools=3)},
        "44bad0fb-1111-2222-3333-444444444444": {
            "run-0": _filho("code-explorer", "44bad0fb-1111-2222-3333-444444444444", tools=1)},
    })
    ags = {a["agentId"]: a for a in list_subagents(pai)}
    assert set(ags) == {"ee6989ae-1db7-4759-ab8c-e3cc9a975ff4",
                        "44bad0fb-1111-2222-3333-444444444444"}
    a = ags["ee6989ae-1db7-4759-ab8c-e3cc9a975ff4"]
    # O tipo do agente so existe no `session_info.name` do PROPRIO filho: enquanto ele roda, o
    # transcript do pai nao tem uma linha dele.
    assert a["agentType"] == "delegate"
    assert a["prompt"] == "Task: confira o MR"
    assert a["toolCalls"] == 3
    assert a["tools"] == [{"name": "bash", "count": 3}]
    assert a["recent"][-1] == {"name": "bash", "target": "git log -2"}
    assert a["lastText"] == "pronto"
    assert a["finished"] is True


@pytest.mark.parametrize("parada,acabou", [
    ("stop", True), ("error", True), ("length", True), ("toolUse", False), (None, False),
])
def test_fim_vem_do_stop_reason_da_ultima_mensagem(raiz_pi, parada, acabou):
    # Sem isto TODO subagente do Pi ficava "rodando" pra sempre: pro Claude quem fecha o cartao e
    # o tool_result no transcript do pai, e o pai do Pi nunca cita o taskId deste subagente.
    pai = _sessao(raiz_pi, {"t1": {"run-0": _filho(
        "delegate", "aaaaaaaa-0000-0000-0000-000000000000", tools=1, parada=parada)}})
    assert list_subagents(pai)[0]["finished"] is acabou


def test_so_a_ultima_retentativa_aparece(raiz_pi):
    # run-1 e uma RETENTATIVA do mesmo task; listar as duas poria a versao abandonada ao lado da
    # que vale (59 destas nesta maquina em 27/08/2026).
    pai = _sessao(raiz_pi, {"t1": {
        "run-0": _filho("delegate", "aaaaaaaa-0000-0000-0000-000000000000", tools=1, texto="velho"),
        "run-1": _filho("delegate", "aaaaaaaa-0000-0000-0000-000000000000", tools=5, texto="novo"),
    }})
    r = list_subagents(pai)
    assert len(r) == 1
    assert r[0]["toolCalls"] == 5 and r[0]["lastText"] == "novo"


def test_detalhe_traz_os_eventos_no_formato_do_chat(raiz_pi):
    pai = _sessao(raiz_pi, {"t1": {
        "run-0": _filho("delegate", "aaaaaaaa-0000-0000-0000-000000000000", tools=2)}})
    a = get_subagent(pai, "t1", 40, 50)
    assert a is not None
    kinds = [e["kind"] for e in a["events"]]
    # Mesmos ChatEvent do chat principal — e o que deixa a tela de Atividade reusar a lista de
    # mensagens em vez de desenhar um formato proprio.
    assert kinds.count("tool_use") == 2 and kinds.count("tool_result") == 2
    assert kinds[-1] == "assistant_msg"


def test_detalhe_recusa_caminho_pra_fora(raiz_pi):
    pai = _sessao(raiz_pi, {"t1": {
        "run-0": _filho("delegate", "aaaaaaaa-0000-0000-0000-000000000000")}})
    assert get_subagent(pai, "../..", 40, 0) is None
    assert get_subagent(pai, "t2", 40, 0) is None


def test_sessao_pi_sem_subagente_devolve_lista_vazia(raiz_pi):
    pai = _sessao(raiz_pi, {})
    assert list_subagents(pai) == []


def test_transcript_fora_da_raiz_do_pi_nao_entra_neste_caminho(tmp_path, monkeypatch):
    # Sem isto, qualquer sessao com uma pasta homonima ao lado do jsonl seria lida como Pi — e o
    # caminho do Claude (<session-dir>/subagents/) deixaria de valer pra ela.
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path / "pi"))
    fora = tmp_path / "outro" / "conversa.jsonl"
    (fora.parent / "conversa" / "t1" / "run-0").mkdir(parents=True)
    fora.write_text("{}\n", encoding="utf-8")
    (fora.parent / "conversa" / "t1" / "run-0" / "session.jsonl").write_text(
        _filho("delegate", "aaaaaaaa-0000-0000-0000-000000000000"), encoding="utf-8")
    assert list_subagents(str(fora)) == []

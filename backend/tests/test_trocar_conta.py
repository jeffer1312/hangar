"""Continuar a mesma conversa noutra conta: o processo para antes de a conversa mudar de conta,
reabre como estava (terminal ou sem terminal) na conta nova, e volta na de origem se não der pra mover."""
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from fastapi.testclient import TestClient

from app.adapters.claude_headless import sessions as S
from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter
from app.config import ConfigDirInfo, settings
from app.models import SessionInfo
from app.registry import sanitize_cwd

SID = "22222222-2222-2222-2222-222222222222"
_H = {"Authorization": "Bearer secret"}


@pytest.fixture
def contas(tmp_path, monkeypatch):
    import app.api as api_mod
    import app.archive as archive_mod
    a, b = tmp_path / ".claude-a", tmp_path / ".claude-b"
    for c in (a, b):
        (c / "projects").mkdir(parents=True)
    lista = [ConfigDirInfo(path=str(a), label="a", active=True), ConfigDirInfo(path=str(b), label="b", active=False)]
    monkeypatch.setattr(api_mod.engines, "caminho", lambda: tmp_path / "engines.json")
    api_mod.engines.salvar("proxy", {"base_url": "http://127.0.0.1:8317", "api_key": "test", "model": "gpt-5.5"})
    monkeypatch.setattr(api_mod, "list_config_dirs", lambda ordered=True: lista)
    monkeypatch.setattr(archive_mod, "list_config_dirs", lambda ordered=True: lista)
    monkeypatch.setattr("app.cotas.cotas_claude", lambda: [])
    monkeypatch.setattr(settings, "projects_dir", a / "projects")
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(S, "_trocando", {})
    settings.auth_token = "secret"
    return str(a), str(b)


def _conversa(conta: str, cwd: str) -> Path:
    jsonl = Path(conta) / "projects" / sanitize_cwd(cwd) / f"{SID}.jsonl"
    jsonl.parent.mkdir(parents=True, exist_ok=True)
    jsonl.write_text("{}\n")
    (jsonl.parent / SID).mkdir()
    return jsonl


def _post(name, destino, *, headless, conta, hl, **extra):
    import app.api as api_mod
    info = SessionInfo(name=name, cwd="/tmp", provider="claude", headless=headless, conta=f"claude:{conta}")
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=headless), \
         patch("app.api._motivo_ocupada", AsyncMock(return_value=None)), \
         patch("app.api.get_adapter", return_value=hl), \
         patch.object(api_mod.registry, "_forget"), \
         patch.object(api_mod.perm_mode, "ler_modo", return_value="manual"), \
         patch.object(api_mod.registry, "para_headless", extra.get("ida")), \
         patch.object(api_mod.registry, "para_terminal", extra.get("volta")):
        return TestClient(api_mod.app).post(f"/api/sessions/{name}/conta", headers=_H, json={"config_dir": destino})


def _hl(ordem):
    hl = ClaudeHeadlessAdapter()
    hl.parar = AsyncMock(side_effect=lambda n: ordem.append("parou"))
    hl.acordar = MagicMock(side_effect=lambda n: ordem.append(("acordou", S.load(n)["config_dir"])))
    hl.ensure_running = AsyncMock(return_value=MagicMock())
    return hl


def test_fixed_proxy_can_return_to_claude_in_same_config_dir(contas, tmp_path):
    import app.api as api_mod
    a, _ = contas
    cwd = str(tmp_path / "repo")
    S.save("hl", cwd, SID, config_dir=a, engine="proxy", model="fixed/gpt-5.5",
           engine_account="default", engine_credential_id="codex:/tmp/codex", context_window=400000)
    source = _conversa(a, cwd)
    info = SessionInfo(name="hl", provider="claude", headless=True, engine="proxy",
                       engine_account="default", conta="codex:/tmp/codex")
    hl = _hl([])
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api._motivo_ocupada", AsyncMock(return_value=None)), \
         patch("app.api.get_adapter", return_value=hl), \
         patch("app.api.cliproxy.is_local_engine", return_value=True), \
         patch.object(api_mod.registry, "_forget"):
        r = TestClient(api_mod.app).post("/api/sessions/hl/conta", headers=_H, json={"config_dir": a})
    assert r.status_code == 200, r.text
    assert source.exists()
    assert all(S.load("hl").get(key) is None for key in (
        "engine", "engine_account", "engine_credential_id", "model", "context_window"))
    hl.parar.assert_awaited_once()


@pytest.mark.parametrize("tier", ["default", "priority"])
@pytest.mark.parametrize("fixed", [False, True])
def test_proxy_fast_reopens_without_changing_identity(contas, tmp_path, tier, fixed):
    import app.api as api_mod
    a, _ = contas
    meta = S.save("hl", str(tmp_path), SID, config_dir=a, engine="proxy", model="fixed/gpt-5.5" if fixed else "gpt-5.5",
                  engine_account="default" if fixed else None, permission_mode="acceptEdits",
                  context_window=400000, effort="high")
    source = _conversa(a, str(tmp_path))
    info = SessionInfo(name="hl", provider="claude", headless=True, engine="proxy",
                       engine_account=meta.get("engine_account"))
    hl = _hl([])
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api._motivo_ocupada", AsyncMock(return_value=None)), \
         patch("app.api.get_adapter", return_value=hl), \
         patch("app.api.cliproxy.supports_fast", return_value=True), \
         patch.object(api_mod.registry, "_forget"), \
         patch("app.api.move_conversation", AsyncMock()) as move:
        response = TestClient(api_mod.app).post("/api/sessions/hl/service-tier", headers=_H,
                                               json={"service_tier": tier})
    assert response.status_code == 200, response.text
    assert response.json() == {"ok": True, "service_tier": tier}
    assert S.load("hl") == {**meta, "service_tier": tier, "problema": None}
    assert source.exists()
    move.assert_not_awaited()
    hl.parar.assert_awaited_once()
    hl.ensure_running.assert_awaited_once()


def test_proxy_fast_reopen_failure_restores_previous_tier(contas, tmp_path):
    import app.api as api_mod
    a, _ = contas
    S.save("hl", str(tmp_path), SID, config_dir=a, engine="proxy", model="gpt-5.5")
    S.update("hl", service_tier="default")
    source = _conversa(a, str(tmp_path))
    info = SessionInfo(name="hl", provider="claude", headless=True, engine="proxy")
    hl = _hl([])
    hl.ensure_running.side_effect = [RuntimeError("failed spawn"), MagicMock()]
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api._motivo_ocupada", AsyncMock(return_value=None)), \
         patch("app.api.get_adapter", return_value=hl), \
         patch("app.api.cliproxy.supports_fast", return_value=True), \
         patch.object(api_mod.registry, "_forget"):
        response = TestClient(api_mod.app).post("/api/sessions/hl/service-tier", headers=_H,
                                               json={"service_tier": "priority"})
    assert response.status_code == 409, response.text
    assert S.load("hl")["service_tier"] == "default"
    assert source.exists()
    assert hl.ensure_running.await_count == 2


@pytest.mark.parametrize("reason", ["erro_sessao_trabalhando", "erro_fila_pendente"])
def test_proxy_fast_does_not_restart_busy_session(contas, tmp_path, reason):
    import app.api as api_mod
    a, _ = contas
    S.save("hl", str(tmp_path), SID, config_dir=a, engine="proxy", model="gpt-5.5")
    info = SessionInfo(name="hl", provider="claude", headless=True, engine="proxy")
    hl = _hl([])
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api._motivo_ocupada", AsyncMock(return_value=reason)), \
         patch("app.api.get_adapter", return_value=hl), \
         patch("app.api.cliproxy.supports_fast", return_value=True):
        response = TestClient(api_mod.app).post("/api/sessions/hl/service-tier", headers=_H,
                                               json={"service_tier": "priority"})
    assert response.status_code == 409
    assert response.json()["detail"]["code"] == reason
    hl.parar.assert_not_awaited()
    assert S.load("hl").get("service_tier") is None


@pytest.mark.parametrize("state,flags,queue,valid,reason", [
    ("working", {"in_progress": True}, [], True, "erro_sessao_trabalhando"),
    ("awaiting_input", {"pending": {}}, [], True, "erro_sessao_esperando_resposta"),
    ("idle", {}, [{"delivered": False, "confirmed": True}], True, "erro_fila_pendente"),
    ("idle", {}, [{"delivered": True, "confirmed": False}], True, None),
    ("idle", {}, [{"delivered": True, "desistiu": True}], True, None),
    ("idle", {}, [{"delivered": True, "confirmed": True, "papel": "assistant"}], True, None),
    (None, {}, [], True, "erro_sessao_iniciando"),
    ("idle", {}, [], False, "erro_sessao_iniciando"),
    ("idle", {}, [], True, None),
])
def test_proxy_fast_checks_preserved_rust_view_after_owner_change(tmp_path, state, flags, queue, valid, reason):
    import asyncio
    import app.api as api_mod
    from app.runtime_adapter import runtime_data
    from app.runtime_coordinator import Binding, Phase, RuntimeCoordinator, Slot

    binding = Binding(name="hl", key=SID, provider="claude", headless=True, meta={}, jsonl="",
                      projection_dir=tmp_path, state_path=tmp_path / "state.json",
                      lock_path=tmp_path / "lock", generation=1)
    coordinator = RuntimeCoordinator()
    coordinator.names["hl"] = SID
    coordinator.slots[SID] = Slot(binding=binding, phase=Phase.Python, change_from_rust=True,
                                  cache_valid=valid, view={"view": {
                                      "initialized": True, "public_state": {"state": state}, **flags,
                                  }})
    with patch("app.runtime_coordinator.current", return_value=coordinator), \
         patch("app.api.PromptQueue") as queues:
        queues.return_value.load.return_value = queue
        assert runtime_data("hl") is None
        assert asyncio.run(api_mod._motivo_ocupada("hl", True)) == reason


@pytest.mark.parametrize("current,expected", [("gpt-6.1-sol", "gpt-6.1-sol"), (None, "gpt-5.5")])
def test_proxy_fast_selection_uses_current_terminal_model(contas, current, expected):
    import app.api as api_mod
    with patch.object(api_mod.registry, "_pane_of", return_value={"pid": 11, "cwd": "/tmp"}), \
         patch.object(api_mod.registry, "resolve_tracked", return_value=(f"/tmp/{SID}.jsonl", True)), \
         patch("app.registry._pid_do_agente", return_value=12), \
         patch("app.procinfo._model_of", return_value=("gpt-5.5", "high")), \
         patch("app.procinfo._env_var_of", return_value="priority"), \
         patch("app.registry._escolhas_status", return_value=(current, "high")), \
         patch("app.registry._claude_reading", side_effect=AssertionError("modelo histórico não é escolha atual")):
        assert api_mod._engine_fast_selection("terminal") == (expected, "priority")


def test_proxy_account_move_preserves_storage_permission_and_conversation(contas, tmp_path):
    import app.api as api_mod
    a, _ = contas
    cwd = str(tmp_path / "repo")
    S.save("hl", cwd, SID, config_dir=a, engine="proxy", model="old/gpt-5.5",
           engine_account="default", permission_mode="acceptEdits")
    source = _conversa(a, cwd)
    info = SessionInfo(name="hl", provider="claude", headless=True, engine="proxy",
                       engine_account="default", conta="codex:/tmp/codex")
    hl = _hl([])
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api._motivo_ocupada", AsyncMock(return_value=None)), \
         patch("app.api.get_adapter", return_value=hl), \
         patch("app.api._fixed_engine_account", return_value={"account": "second", "prefix": "new",
                                                           "credential_id": "codex:/tmp/second", "home": "/tmp/second",
                                                           "base_url": "http://127.0.0.1:8317"}), \
         patch("app.api._engine_models", AsyncMock(return_value=[{"id": "new/gpt-5.5"}])), \
         patch("app.api.engines.env_de", return_value={}), \
         patch.object(api_mod.registry, "_forget"):
        r = TestClient(api_mod.app).post("/api/sessions/hl/conta", headers=_H,
                                       json={"engine_account": "second"})
    assert r.status_code == 200, r.text
    meta = S.load("hl")
    assert meta["model"] == "new/gpt-5.5" and meta["engine_account"] == "second"
    assert meta["config_dir"] == a and meta["permission_mode"] == "acceptEdits"
    assert source.exists() and meta["session_id"] == SID


def test_fixed_model_reopens_same_account_and_preserves_permission(contas, tmp_path):
    import app.api as api_mod
    a, _ = contas
    S.save("hl", str(tmp_path), SID, config_dir=a, engine="proxy", model="fixed/gpt-old",
           engine_account="default", engine_credential_id="codex:/tmp/codex",
           permission_mode="acceptEdits")
    source = _conversa(a, str(tmp_path))
    hl = _hl([])
    info = SessionInfo(name="hl", jsonl=str(source), provider="claude", headless=True,
                       engine="proxy", engine_account="default", conta="codex:/tmp/codex")
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api._motivo_ocupada", AsyncMock(return_value=None)), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.api.get_adapter", return_value=hl), \
         patch("app.api._fixed_engine_account", return_value={"account": "default", "prefix": "fixed",
                                                           "credential_id": "codex:/tmp/codex", "home": "/tmp/codex",
                                                           "base_url": "http://127.0.0.1:8317"}), \
         patch("app.api._fixed_engine_models", AsyncMock(return_value=[{"id": "gpt-new", "context_length": 400000}])), \
         patch("app.api._engine_models", AsyncMock(return_value=[{"id": "fixed/gpt-new", "context_length": 400000}])), \
         patch("app.api.engines.env_de", return_value={}), \
         patch.object(api_mod.registry, "_forget"):
        r = TestClient(api_mod.app).post("/api/sessions/hl/engine/model", headers=_H,
                                       json={"model": "gpt-new", "effort": "high"})
    assert r.status_code == 200 and r.json() == {"ok": True, "model": "gpt-new"}, r.text
    meta = S.load("hl")
    assert meta["model"] == "fixed/gpt-new" and meta["engine_account"] == "default"
    assert meta["context_window"] == 400000 and meta["effort"] == "high"
    assert meta["permission_mode"] == "acceptEdits" and meta["session_id"] == SID
    assert source.exists()
    hl.parar.assert_awaited_once()
    hl.ensure_running.assert_awaited_once()


def test_proxy_reopen_failure_restores_selection(contas, tmp_path):
    import app.api as api_mod
    a, _ = contas
    S.save("hl", str(tmp_path), SID, config_dir=a, engine="proxy", model="old/gpt-5.5",
           engine_account="default", permission_mode="acceptEdits")
    _conversa(a, str(tmp_path))
    hl = _hl([])
    hl._subidas["hl"] = 3
    hl.ensure_running.side_effect = [RuntimeError("failed spawn"), MagicMock()]
    info = SessionInfo(name="hl", provider="claude", headless=True, engine="proxy",
                       engine_account="default")
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api._motivo_ocupada", AsyncMock(return_value=None)), \
         patch("app.api.get_adapter", return_value=hl), \
         patch("app.api._fixed_engine_account", return_value={"account": "second", "prefix": "new",
                                                           "credential_id": "codex:/tmp/second", "home": "/tmp/second",
                                                           "base_url": "http://127.0.0.1:8317"}), \
         patch("app.api._engine_models", AsyncMock(return_value=[{"id": "new/gpt-5.5"}])), \
         patch("app.api.engines.env_de", return_value={}), \
         patch.object(api_mod.registry, "_forget"):
        r = TestClient(api_mod.app).post("/api/sessions/hl/conta", headers=_H,
                                       json={"engine_account": "second"})
    assert r.status_code == 409, r.text
    assert S.load("hl")["engine_account"] == "default"
    assert S.load("hl")["model"] == "old/gpt-5.5"
    assert hl.ensure_running.await_count == 2
    assert "hl" not in hl._subidas
    assert all(call.kwargs["require_initialize"] for call in hl.ensure_running.await_args_list)


def test_stale_engine_identity_cannot_overwrite_current_claude(contas, tmp_path):
    import app.api as api_mod
    a, _ = contas
    S.save("hl", str(tmp_path), SID, config_dir=a, model="sonnet")
    hl = _hl([])
    stale_info = SessionInfo(name="hl", provider="claude", headless=True, engine="proxy",
                             engine_account="default")
    with patch("app.api._cached_info", AsyncMock(return_value=stale_info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api.get_adapter", return_value=hl):
        r = TestClient(api_mod.app).post("/api/sessions/hl/conta", headers=_H,
                                       json={"engine_account": "second"})
    assert r.status_code == 400, r.text
    assert S.load("hl")["engine"] is None and S.load("hl")["model"] == "sonnet"
    hl.parar.assert_not_awaited()


def test_sem_terminal_para_move_e_religa_na_conta_nova(contas, tmp_path):
    a, b = contas
    cwd = str(tmp_path / "repo")
    S.save("hl", cwd, SID, config_dir=a)
    origem = _conversa(a, cwd)
    ordem = []
    r = _post("hl", b, headless=True, conta=a, hl=_hl(ordem))
    assert r.status_code == 200 and r.json() == {"ok": True, "config_dir": b}
    destino = Path(b) / "projects" / origem.parent.name
    assert (destino / f"{SID}.jsonl").exists() and (destino / SID).is_dir() and not origem.exists()
    assert ordem == ["parou", ("acordou", b)]


def test_unanswered_message_goes_back_to_the_queue_before_the_new_account_wakes(contas, tmp_path):
    """Entregue e nunca confirmada (limite da conta) volta à fila para a conta nova responder;
    desistida, saída local e confirmada ficam como estão."""
    from app.pqueue import PromptQueue
    a, b = contas
    S.save("hl", str(tmp_path / "repo"), SID, config_dir=a)
    _conversa(a, str(tmp_path / "repo"))
    queue = PromptQueue("hl")
    lost = queue.append("resposta perdida", delivered=True)
    abandoned = queue.append("desistida", delivered=True)
    queue.desistir(abandoned["id"])
    local = queue.append_saida_local("/btw isn't available")
    done = queue.append("respondida", delivered=True)
    queue.confirm_delivered(lambda r: r["id"] == done["id"])
    seen = []
    hl = _hl([])
    hl.acordar = MagicMock(side_effect=lambda n: seen.extend(queue.load()))
    r = _post("hl", b, headless=True, conta=a, hl=hl)
    assert r.status_code == 200, r.text
    by_id = {row["id"]: row for row in seen}
    # A antiga sai (o drain com o id dela devolveria a operação já aceita sem enviar); nasce uma nova.
    assert by_id[lost["id"]]["confirmed"] is True
    fresh = [row for row in seen if row["text"] == "resposta perdida" and row["id"] != lost["id"]]
    assert len(fresh) == 1 and fresh[0]["delivered"] is False
    assert by_id[abandoned["id"]].get("confirmed") is not True and by_id[abandoned["id"]]["desistiu"]
    assert by_id[local["id"]]["delivered"] and by_id[done["id"]]["confirmed"]


def test_unanswered_already_in_transcript_is_confirmed_not_resent(monkeypatch):
    """Confirmação atrasada (o texto já está no transcript) não pode virar mensagem repetida."""
    from types import SimpleNamespace
    import app.api as api_mod
    rows = [{"id": "landed", "text": "chegou", "delivered": True},
            {"id": "lost", "text": "perdida", "delivered": True}]
    appended, confirmed = [], []

    class Queue:
        def __init__(self, name):
            pass

        def load(self):
            return rows

        def append(self, text, pre_transcript=False):
            appended.append(text)

        def confirm_delivered(self, apenas):
            confirmed.extend(r["id"] for r in rows if apenas(r))

    monkeypatch.setattr(api_mod, "PromptQueue", Queue)
    monkeypatch.setattr(api_mod.headless_sessions, "load", lambda name: {})
    monkeypatch.setattr(api_mod, "get_adapter", lambda kind: SimpleNamespace(transcript_path_de=lambda meta: "/x.jsonl"))
    monkeypatch.setattr(api_mod, "committed_user_lines", lambda path: {"chegou"})
    assert api_mod._requeue_unanswered("hl") == 1
    assert appended == ["perdida"] and confirmed == ["landed", "lost"]
    monkeypatch.setattr(api_mod, "committed_user_lines", lambda path: None)
    appended.clear(); confirmed.clear()
    assert api_mod._requeue_unanswered("hl") == 0, "transcript ilegível não autoriza reenviar"
    assert appended == [] and confirmed == []


def test_terminal_passa_por_sem_terminal_e_reabre_o_pane_na_conta_nova(contas, tmp_path):
    a, b = contas
    cwd = str(tmp_path / "repo")
    origem = _conversa(a, cwd)
    visto = {}
    ida = MagicMock(side_effect=lambda n, modo, **_: S.save(n, cwd, SID, config_dir=a, permission_mode=modo))
    volta = MagicMock(side_effect=lambda n: visto.update(conta=S.load(n)["config_dir"], na_origem=origem.exists()))
    r = _post("t1", b, headless=False, conta=a, hl=_hl([]), ida=ida, volta=volta)
    assert r.status_code == 200
    ida.assert_called_once_with("t1", "manual", target_config_dir=b)
    assert visto == {"conta": b, "na_origem": False}


def test_read_only_cujo_terminal_nao_volta_fica_parado_sem_acordar(contas, tmp_path):
    a, b = contas
    cwd = str(tmp_path / "repo")
    _conversa(a, cwd)
    ordem = []
    ida = MagicMock(side_effect=lambda n, modo, **_: S.save(n, cwd, SID, config_dir=a, permission_mode=modo, read_only=True))
    volta = MagicMock(side_effect=ValueError("bwrap sumiu"))
    r = _post("t1", b, headless=False, conta=a, hl=_hl(ordem), ida=ida, volta=volta)
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_troca_conta_parada" and r.json()["detail"]["params"]["erro"] == "bwrap sumiu"
    assert ordem == [] and S.load("t1")["read_only"] is True


@pytest.mark.parametrize("headless", [True, False])
def test_account_move_trusts_folder_in_new_account_before_reopening(contas, tmp_path, monkeypatch, headless):
    import app.api as api_mod
    a, b = contas
    cwd = str(tmp_path / "repo")
    S.save("s1", cwd, SID, config_dir=a)
    ordem = []
    monkeypatch.setattr(api_mod.registry_mod, "_pretrust_cwd", lambda pasta, conta: ordem.append((pasta, conta)))
    volta = MagicMock(side_effect=lambda n: ordem.append(("acordou", b)))
    r = _post("s1", b, headless=headless, conta=a, hl=_hl(ordem), ida=MagicMock(), volta=volta)
    assert r.status_code == 200
    assert ordem[-2:] == [(cwd, b), ("acordou", b)]


def test_conta_destino_com_a_mesma_conversa_reabre_na_origem(contas, tmp_path):
    a, b = contas
    cwd = str(tmp_path / "repo")
    S.save("hl", cwd, SID, config_dir=a)
    origem = _conversa(a, cwd)
    _conversa(b, cwd)
    ordem = []
    r = _post("hl", b, headless=True, conta=a, hl=_hl(ordem))
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_conversa_ja_na_conta"
    assert origem.exists() and ordem == ["parou", ("acordou", a)]


def test_sidecar_que_nao_grava_devolve_a_conversa_para_a_origem(contas, tmp_path, monkeypatch):
    a, b = contas
    cwd = str(tmp_path / "repo")
    S.save("hl", cwd, SID, config_dir=a)
    origem = _conversa(a, cwd)
    real_update = S.update
    monkeypatch.setattr(S, "update", lambda name, **campos: None if "config_dir" in campos else real_update(name, **campos))
    ordem = []
    r = _post("hl", b, headless=True, conta=a, hl=_hl(ordem))
    assert r.status_code == 500 and r.json()["detail"]["code"] == "erro_mover_conversa"
    assert origem.exists() and not (Path(b) / "projects" / origem.parent.name / f"{SID}.jsonl").exists()
    assert ordem == ["parou", ("acordou", a)]


def test_conta_perto_do_limite_e_recusada_e_vai_para_o_fim_da_lista(contas, tmp_path, monkeypatch):
    import app.api as api_mod
    from app.cotas import CotaConta, JanelaCota
    a, b = contas
    c = str(tmp_path / ".claude-c")
    lista = api_mod.list_config_dirs() + [ConfigDirInfo(path=c, label="c", active=False)]
    monkeypatch.setattr(api_mod, "list_config_dirs", lambda ordered=True: lista)
    cheia = CotaConta(id=f"claude:{b}", label="b", provedor="claude", estado="lida",
                      janelas=[JanelaCota(rotulo="5h", pct=20), JanelaCota(rotulo="7d", pct=99)])
    acabando = CotaConta(id=f"claude:{c}", label="c", provedor="claude", estado="lida", janelas=[JanelaCota(rotulo="5h", pct=96)])
    monkeypatch.setattr("app.cotas.cotas_claude", lambda: [cheia, acabando])
    assert [(d["label"], d["low"], d["full"]) for d in api_mod._account_targets(a)] == [("c", True, False), ("b", True, True)]
    S.save("hl", str(tmp_path / "repo"), SID, config_dir=a)
    ordem = []
    r = _post("hl", b, headless=True, conta=a, hl=_hl(ordem))
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_conta_cheia" and ordem == []


def test_conta_fora_da_lista_e_recusada(contas):
    import app.api as api_mod
    r = TestClient(api_mod.app).post("/api/sessions/hl/conta", headers=_H, json={"config_dir": "/etc"})
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_config_dir_invalido"


@pytest.mark.parametrize("teimoso", [False, True])
def test_windows_mata_as_sobras_numa_chamada_so_e_sem_arvore(monkeypatch, teimoso):
    """Os netos que o psmux não derruba morrem logo, num taskkill só; sem /T, que seguiria o ppid
    de um pid reaproveitado. Quem sobrevive ao taskkill faz a troca falhar."""
    from types import SimpleNamespace
    import app.api as api_mod
    vivos, chamadas, esperas = {11, 12}, [], []
    monkeypatch.setattr(api_mod, "os", SimpleNamespace(name="nt"))
    monkeypatch.setattr(api_mod.shutil, "which", lambda n: r"C:\Windows\System32\taskkill.exe")
    monkeypatch.setattr(api_mod.procinfo, "pid_vivo", lambda p: p in vivos)
    monkeypatch.setattr(api_mod.registry_mod, "_esperar_saida", lambda pids, teto: esperas.append(teto))

    def run(argv, **kw):
        chamadas.append(argv)
        if not teimoso:
            vivos.clear()
        return SimpleNamespace(returncode=128 if teimoso else 0, stdout="", stderr="Acesso negado." if teimoso else "")
    monkeypatch.setattr(api_mod.subprocess, "run", run)

    assert api_mod._saiu([10, 11, 12]) is not teimoso
    assert chamadas == [[r"C:\Windows\System32\taskkill.exe", "/F", "/PID", "11", "/PID", "12"]]
    assert esperas[0] < 15


def test_windows_sem_taskkill_nao_finge_que_matou(monkeypatch):
    from types import SimpleNamespace
    import app.api as api_mod
    monkeypatch.setattr(api_mod, "os", SimpleNamespace(name="nt"))
    monkeypatch.setattr(api_mod.shutil, "which", lambda n: None)
    monkeypatch.setattr(api_mod.procinfo, "pid_vivo", lambda p: p == 11)
    monkeypatch.setattr(api_mod.registry_mod, "_esperar_saida", lambda pids, teto: None)
    monkeypatch.setattr(api_mod.subprocess, "run", lambda *a, **kw: pytest.fail("rodou sem taskkill"))
    assert api_mod._saiu([10, 11]) is False

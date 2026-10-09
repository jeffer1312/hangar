"""Troca terminal ⇄ sem terminal na mesma conversa: ordem (antigo morre antes do novo), o que
passa de um lado pro outro, a volta quando o novo não nasce, e o 409 com a sessão ocupada."""
import asyncio
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from fastapi.testclient import TestClient

from app.adapters.claude_headless import sessions as S
from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter
from app.config import settings

SID = "11111111-1111-1111-1111-111111111111"


@pytest.fixture
def reg(tmp_path, monkeypatch):
    from app import pqueue, registry as R
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(S, "_trocando", {})
    monkeypatch.setattr(R.procinfo, "pid_vivo", lambda pid: False)
    monkeypatch.setattr(R.tmux, "has_session", lambda n: False)
    return R.SessionRegistry(str(tmp_path / "projects"))


def test_sem_terminal_vira_pane_com_resume_e_as_escolhas(reg, tmp_path, monkeypatch):
    from app import registry as R
    from app.pqueue import PromptQueue
    meta = S.save("hl", str(tmp_path), SID, model="haiku", effort="low", permission_mode="acceptEdits",
                  subagent_model="sonnet")
    jsonl = ClaudeHeadlessAdapter().transcript_path_de(meta)
    (tmp_path / "projects").mkdir(exist_ok=True)
    from pathlib import Path
    Path(jsonl).parent.mkdir(parents=True, exist_ok=True)
    Path(jsonl).write_text("{}\n")
    PromptQueue("hl").append_saida_local("nota que fica")
    ordem = []
    fake_hl = MagicMock()
    fake_hl.transcript_path_de.side_effect = ClaudeHeadlessAdapter().transcript_path_de
    fake_hl.close_sync.side_effect = lambda n, m: ordem.append(("fechou", S.exists(n)))
    fake_hl.escolhas.return_value = (None, None)
    monkeypatch.setattr("app.adapters.get_adapter", lambda chave: fake_hl)

    def nova(name, cwd, cmd, cfg=None, provider="claude", env=None):
        ordem.append(("pane", cmd))
        ordem.append(("env", env))
        return True
    monkeypatch.setattr(R.tmux, "new_session", nova)
    info = reg.para_terminal("hl")
    assert ordem[0] == ("fechou", False)            # sidecar some antes de matar o processo
    cmd = ordem[1][1]
    assert f"--resume {SID}" in cmd and "--model haiku" in cmd and "--effort low" in cmd
    assert "--permission-mode acceptEdits" in cmd
    assert ordem[2][0] == "env" and ordem[2][1]["CLAUDE_CODE_SUBAGENT_MODEL"] == "sonnet"
    assert not S.exists("hl") and not info.headless and info.jsonl == jsonl
    assert S.em_troca("hl")
    assert any(e.get("text") == "nota que fica" for e in PromptQueue("hl").load())


@pytest.mark.parametrize("resume", [False, True])
def test_terminal_command_keeps_plugins_and_choices(monkeypatch, resume):
    import shlex
    from app import plugin_bridge, registry as R
    roots = ["/plugins/hangar", "/plugins/outro mod"]
    monkeypatch.setattr(plugin_bridge, "raizes_dos_plugins", lambda: roots)
    meta = {"session_id": SID, "cwd": "/projeto", "model": "haiku", "effort": "low",
            "permission_mode": "acceptEdits"}
    argv = shlex.split(R.SessionRegistry._comando_terminal(meta, resume=resume))
    assert argv[:3] == ["claude", "--resume" if resume else "--session-id", SID]
    assert [argv[i + 1] for i, value in enumerate(argv[:-1]) if value == "--plugin-dir"] == roots
    assert argv[argv.index("--model") + 1] == "haiku"
    assert argv[argv.index("--effort") + 1] == "low"
    assert argv[argv.index("--permission-mode") + 1] == "acceptEdits"


def test_pane_que_nao_nasce_devolve_o_sidecar(reg, tmp_path, monkeypatch):
    from app import registry as R
    S.save("hl", str(tmp_path), SID, model="haiku")
    monkeypatch.setattr("app.adapters.get_adapter", lambda chave: MagicMock(
        transcript_path_de=lambda m: str(tmp_path / "nao-existe.jsonl"), escolhas=lambda n: (None, None)))
    monkeypatch.setattr(R.tmux, "new_session", lambda *a, **k: False)
    with pytest.raises(ValueError):
        reg.para_terminal("hl")
    assert S.load("hl")["session_id"] == SID and S.load("hl")["cano"] is None


def _pane(reg, tmp_path, monkeypatch, *, mata=True):
    from app import registry as R
    jsonl = str(tmp_path / "projects" / "x" / f"{SID}.jsonl")
    monkeypatch.setattr(reg, "_pane_of", lambda n: {"name": n, "cwd": str(tmp_path), "pid": 999})
    monkeypatch.setattr(R.SessionRegistry, "_refuse_non_claude_resume", staticmethod(lambda p: None))
    monkeypatch.setattr(reg, "resolve_tracked", lambda n, c: (jsonl, True))
    monkeypatch.setattr(R, "_config_dir_of", lambda pid: tmp_path / ".claude-b")
    monkeypatch.setattr(R, "_engine_of", lambda pid: None)
    monkeypatch.setattr(R.procinfo, "_model_of", lambda pid: ("sonnet", "high"))
    monkeypatch.setattr(R.procinfo, "_env_var_of",
                        lambda pid, n: "haiku" if n == "CLAUDE_CODE_SUBAGENT_MODEL" else None)
    monkeypatch.setattr(R, "_descendant_pids", lambda pid: [999, 1000])
    monkeypatch.setattr(R, "agente_do_pane", lambda pid, children=None: ("claude", 999))
    monkeypatch.setattr(R, "_escolhas_status", lambda sid: (None, None))
    mortos = []

    def kill(n):
        mortos.append((n, S.exists(n)))
        return mata
    monkeypatch.setattr(R.tmux, "kill_session", kill)
    return mortos


def test_pane_vira_sem_terminal_com_sid_vivo_conta_e_modo(reg, tmp_path, monkeypatch):
    mortos = _pane(reg, tmp_path, monkeypatch)
    meta = reg.para_headless("t1", "plan")
    assert mortos == [("t1", False)]                 # pane morre antes do sidecar existir
    assert meta["session_id"] == SID and S.load("t1")["session_id"] == SID
    assert meta["config_dir"] == str(tmp_path / ".claude-b")
    assert (meta["model"], meta["effort"], meta["permission_mode"]) == ("sonnet", "high", "plan")
    assert meta["subagent_model"] == "haiku"


@pytest.mark.parametrize("tier", ["default", "priority"])
@pytest.mark.parametrize("rollback", [False, True])
@pytest.mark.parametrize("fixed_account", [False, True])
@pytest.mark.parametrize("live_model", ["gpt-6.1-sol", None])
def test_engine_tier_survives_transport_and_rollback(reg, tmp_path, monkeypatch, tier, rollback, fixed_account, live_model):
    from app import registry as R, engines, cliproxy, cliproxy_accounts
    stopped = _pane(reg, tmp_path, monkeypatch)
    monkeypatch.setattr(engines, "caminho", lambda: tmp_path / "engines.json")
    engines.salvar("proxy", {"base_url": "http://127.0.0.1:8317", "api_key": "test", "model": "gpt-5.5"})
    monkeypatch.setattr(R, "_engine_of", lambda pid: "proxy")
    monkeypatch.setattr(R.procinfo, "_model_of", lambda pid: ("gpt-5.5", "high"))
    monkeypatch.setattr(R, "_escolhas_status", lambda sid: (live_model, "high"))
    monkeypatch.delenv("CLAUDE_CODE_EXTRA_BODY", raising=False)
    historical = tmp_path / "projects" / "x" / f"{SID}.jsonl"
    historical.parent.mkdir(parents=True, exist_ok=True)
    historical.write_text('{"type":"assistant","message":{"model":"gpt-4","usage":{"input_tokens":100}}}', encoding="utf-8")
    monkeypatch.setattr(cliproxy, "supports_fast", lambda *a: True)
    monkeypatch.setattr(R, "_exigir_cp_engine", lambda: None)
    account = {"account": "default", "prefix": "fixed", "credential_id": "codex:/tmp/codex",
               "home": "/tmp/codex", "base_url": "http://127.0.0.1:8317"}
    monkeypatch.setattr(cliproxy, "account_for_engine", lambda *a, **k: account)
    monkeypatch.setattr(cliproxy_accounts, "resolve", lambda *a, **k: account)
    monkeypatch.setattr("app.engine_probe.listar_modelos", lambda *a: [{"id": "fixed/gpt-5.5"}, {"id": "fixed/gpt-6.1-sol"}])
    markers = {"CP_ENGINE_SERVICE_TIER": tier}
    if fixed_account:
        markers.update(CP_ENGINE_ACCOUNT="default", CP_ENGINE_CREDENTIAL_ID="codex:/tmp/codex",
                       CP_ENGINE_ACCOUNT_BASE_URL="http://127.0.0.1:8317")
    expected_model = ("fixed/" if fixed_account else "") + (live_model or "gpt-5.5")
    def read(pid, name):
        assert not stopped
        return markers.get(name)
    monkeypatch.setattr(R.procinfo, "_env_var_of", read)
    commands = []
    monkeypatch.setattr(R.tmux, "new_session", lambda name, cwd, cmd, *a, **k: commands.append(cmd) or True)
    if rollback:
        monkeypatch.setattr(S, "save", MagicMock(side_effect=OSError("sidecar indisponível")))
        with pytest.raises(OSError, match="sidecar indisponível"):
            reg.para_headless("t1", "acceptEdits")
    else:
        meta = reg.para_headless("t1", "acceptEdits")
        assert meta["service_tier"] == tier and S.load("t1")["service_tier"] == tier
        assert meta["model"] == expected_model and S.load("t1")["model"] == expected_model
        fake_hl = MagicMock(transcript_path_de=lambda m: str(tmp_path / "missing.jsonl"))
        monkeypatch.setattr("app.adapters.get_adapter", lambda chave: fake_hl)
        reg.para_terminal("t1")
    assert stopped == [("t1", False)]
    assert f"--service-tier {tier} -- claude" in commands[0]
    assert f"--model {expected_model}" in commands[0] and "--model gpt-4" not in commands[0]
    if live_model is not None:
        assert "--model gpt-5.5" not in commands[0]
    if fixed_account:
        assert "--account default --account-home /tmp/codex" in commands[0]


@pytest.mark.parametrize("source", ["shell", "user", "project", "local"])
def test_invalid_engine_body_keeps_headless_before_terminal(reg, tmp_path, monkeypatch, source):
    import json
    import tempfile
    from app import registry as R, engines, cliproxy
    cwd = tmp_path / "session"
    cwd.mkdir()
    config = tmp_path / "account"
    config.mkdir()
    operator = tmp_path / "operator"
    operator.mkdir()
    monkeypatch.chdir(operator)
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(operator / "wrong-account"))
    monkeypatch.delenv("CLAUDE_CODE_EXTRA_BODY", raising=False)
    monkeypatch.setattr(engines, "caminho", lambda: tmp_path / "engines.json")
    engines.salvar("proxy", {"base_url": "http://127.0.0.1:8317", "api_key": "test", "model": "gpt-5.5"})
    monkeypatch.setattr(cliproxy, "supports_fast", lambda *a: True)
    monkeypatch.setattr(R, "_exigir_cp_engine", lambda: None)
    if source == "shell":
        monkeypatch.setenv("CLAUDE_CODE_EXTRA_BODY", "broken")
    else:
        path = config / "settings.json" if source == "user" else cwd / ".claude" / (
            "settings.local.json" if source == "local" else "settings.json")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"env": {"CLAUDE_CODE_EXTRA_BODY": "broken"}}), encoding="utf-8")
    meta = S.save("hl", str(cwd), SID, config_dir=str(config), engine="proxy",
                  model="gpt-5.5", service_tier="priority")
    fake_hl = MagicMock(transcript_path_de=lambda m: str(cwd / "missing.jsonl"))
    monkeypatch.setattr("app.adapters.get_adapter", lambda key: fake_hl)
    new_session = MagicMock()
    monkeypatch.setattr(R.tmux, "new_session", new_session)
    private_file = MagicMock()
    monkeypatch.setattr(tempfile, "mkstemp", private_file)
    with pytest.raises(ValueError, match="CLAUDE_CODE_EXTRA_BODY"):
        reg.para_terminal("hl")
    assert S.load("hl") == meta and not S.em_troca("hl")
    assert R.Path.cwd() == operator
    fake_hl.close_sync.assert_not_called()
    new_session.assert_not_called()
    private_file.assert_not_called()


@pytest.mark.parametrize("removed", [False, True])
def test_transport_incompatible_priority_refuses_before_kill_or_clears_fallback(reg, tmp_path, monkeypatch, removed):
    from app import registry as R, engines, cliproxy
    stopped = _pane(reg, tmp_path, monkeypatch)
    monkeypatch.setattr(engines, "caminho", lambda: tmp_path / "engines.json")
    engines.salvar("proxy", {"base_url": "http://127.0.0.1:8317", "api_key": "test", "model": "gpt-5.5"})
    monkeypatch.setattr(R, "_engine_of", lambda pid: "removed" if removed else "proxy")
    monkeypatch.setattr(R.procinfo, "_env_var_of", lambda pid, name: "priority" if name == "CP_ENGINE_SERVICE_TIER" else None)
    monkeypatch.setattr(cliproxy, "supports_fast", lambda *a: False)
    if removed:
        meta = reg.para_headless("t1", "acceptEdits")
        assert "service_tier" not in meta and meta["engine"] is None
    else:
        with pytest.raises(ValueError, match="service_tier"):
            reg.para_headless("t1", "acceptEdits")
        assert not stopped and not S.exists("t1")


def test_pane_aberto_no_shell_le_conta_e_escolhas_do_claude_filho(reg, tmp_path, monkeypatch):
    # Sessão aberta no terminal: o pid do pane é o fish, e só o `claude` filho tem a conta e o modelo.
    from app import registry as R
    _pane(reg, tmp_path, monkeypatch)
    so_filho = lambda valor: (lambda pid, *a: valor if pid == 1000 else None)
    monkeypatch.setattr(R, "agente_do_pane", lambda pid, children=None: ("claude", 1000))
    monkeypatch.setattr(R, "_config_dir_of", so_filho(tmp_path / ".claude-b"))
    monkeypatch.setattr(R.procinfo, "_model_of", lambda pid: ("sonnet", "high") if pid == 1000 else (None, None))
    monkeypatch.setattr(R.procinfo, "_env_var_of",
                        lambda pid, key: "haiku" if pid == 1000 and key == "CLAUDE_CODE_SUBAGENT_MODEL" else None)
    meta = reg.para_headless("t1", "plan")
    assert meta["config_dir"] == str(tmp_path / ".claude-b")
    assert (meta["model"], meta["effort"], meta["subagent_model"]) == ("sonnet", "high", "haiku")


def test_pane_leva_o_modelo_em_uso_nao_o_do_boot(reg, tmp_path, monkeypatch):
    # Aberta sem --model (ou trocada por /model na TUI): quem sabe o modelo em uso é a statusline.
    from app import registry as R
    _pane(reg, tmp_path, monkeypatch)
    monkeypatch.setattr(R.procinfo, "_model_of", lambda pid: (None, None))
    monkeypatch.setattr(R, "_escolhas_status", lambda sid: ("claude-opus-5[1m]", "high"))
    meta = reg.para_headless("t1", "plan")
    assert (meta["model"], meta["effort"]) == ("claude-opus-5[1m]", "high")


def test_esforco_que_nao_existe_na_abertura_nao_trava_a_troca(reg, tmp_path, monkeypatch):
    # `ultracode` só existe no /effort em voo; a flag --effort recusa e a troca dava 409.
    from app import registry as R
    _pane(reg, tmp_path, monkeypatch)
    monkeypatch.setattr(R, "_escolhas_status", lambda sid: ("claude-opus-5[1m]", "ultracode"))
    meta = reg.para_headless("t1", "plan")
    assert (meta["model"], meta["effort"]) == ("claude-opus-5[1m]", "high")


def test_sem_terminal_volta_com_o_modelo_em_uso(reg, tmp_path, monkeypatch):
    from app import registry as R
    from pathlib import Path
    meta = S.save("hl", str(tmp_path), SID)
    jsonl = ClaudeHeadlessAdapter().transcript_path_de(meta)
    Path(jsonl).parent.mkdir(parents=True, exist_ok=True)
    Path(jsonl).write_text("{}\n")
    fake_hl = MagicMock()
    fake_hl.transcript_path_de.side_effect = ClaudeHeadlessAdapter().transcript_path_de
    fake_hl.escolhas.return_value = ("claude-opus-5[1m]", "high")
    monkeypatch.setattr("app.adapters.get_adapter", lambda chave: fake_hl)
    cmds = []
    monkeypatch.setattr(R.tmux, "new_session", lambda name, cwd, cmd, *a, **k: cmds.append(cmd) or True)
    reg.para_terminal("hl")
    assert "--model 'claude-opus-5[1m]'" in cmds[0] and "--effort high" in cmds[0]
    fake_hl.escolhas.return_value = ("claude-opus-5[1m]", "ultracode")
    S.save("hl", str(tmp_path), SID)
    reg.para_terminal("hl")
    assert "--effort" not in cmds[1]


def test_pane_que_nao_morre_nao_vira_sem_terminal(reg, tmp_path, monkeypatch):
    from app.registry import KillFailed
    _pane(reg, tmp_path, monkeypatch, mata=False)
    with pytest.raises(KillFailed):
        reg.para_headless("t1", "manual")
    assert not S.exists("t1")


def test_monitor_sem_terminal_nao_diz_dead_durante_a_troca(tmp_path, monkeypatch):
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(S, "_trocando", {})
    S.marcar_troca("hl")
    ad = ClaudeHeadlessAdapter()

    async def fluxo():
        gen = ad.state_monitor("hl", lambda: None)
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(gen.__anext__(), 1.5)
        S._trocando.clear()
        gen = ad.state_monitor("hl", lambda: None)
        assert (await gen.__anext__()).state == "dead"
    asyncio.run(fluxo())


@pytest.fixture
def cliente(monkeypatch):
    import app.api as api_mod
    settings.auth_token = "secret"
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    return TestClient(api_mod.app)


_H = {"Authorization": "Bearer secret"}


@pytest.mark.parametrize("terminal", [True, False])
def test_codex_switches_in_both_directions(cliente, terminal):
    from app.adapters.codex.adapter import CodexAdapter
    from app.models import SessionInfo
    codex = CodexAdapter()
    codex.open_terminal = AsyncMock()
    codex.open_headless = AsyncMock()
    info = SessionInfo(name="cx", cwd="/tmp", provider="codex", headless=terminal)
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api.get_adapter", return_value=codex), \
         patch("app.api.registry._forget"):
        result = cliente.post("/api/sessions/cx/modo-execucao", headers=_H, json={"terminal": terminal})
    assert result.status_code == 200 and result.json() == {"ok": True, "terminal": terminal}
    (codex.open_terminal if terminal else codex.open_headless).assert_awaited_once_with("cx")
    (codex.open_headless if terminal else codex.open_terminal).assert_not_awaited()


def test_troca_com_turno_em_voo_da_409_e_nao_mexe(cliente, monkeypatch):
    from app.models import SessionInfo
    import app.api as api_mod
    info = SessionInfo(name="hl", cwd="/tmp", jsonl="/tmp/x.jsonl", tracked=True, provider="claude", headless=True)
    hl = ClaudeHeadlessAdapter()
    sess = MagicMock(vivo=True, iniciando=False, pending={}, question=None, in_progress=True)
    hl._sessions["hl"] = sess
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=True), \
         patch("app.api.get_adapter", return_value=hl), \
         patch.object(api_mod.registry, "para_terminal") as troca:
        r = cliente.post("/api/sessions/hl/modo-execucao", headers=_H, json={"terminal": True})
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_sessao_trabalhando"
    troca.assert_not_called()


def test_troca_para_sem_terminal_sobe_o_processo_e_volta_se_nao_subir(cliente, monkeypatch, tmp_path):
    from app.models import SessionInfo
    import app.api as api_mod
    monkeypatch.setattr("app.pqueue.settings.projects_dir", tmp_path)
    info = SessionInfo(name="t1", cwd="/tmp", jsonl="/tmp/x.jsonl", tracked=True, provider="claude", state="idle")
    hl = ClaudeHeadlessAdapter()
    hl.ensure_running = AsyncMock(side_effect=RuntimeError("binário não encontrado: claude"))
    with patch("app.api._cached_info", AsyncMock(return_value=info)), \
         patch("app.api._headless", return_value=False), \
         patch("app.api.get_adapter", return_value=hl), \
         patch.object(api_mod.registry, "list_with_state", AsyncMock(return_value=[info])), \
         patch.object(api_mod.perm_mode, "ler_modo", return_value="manual"), \
         patch.object(api_mod.registry, "para_headless", return_value={}) as ida, \
         patch.object(api_mod.registry, "para_terminal") as volta:
        r = cliente.post("/api/sessions/t1/modo-execucao", headers=_H, json={"terminal": False})
    ida.assert_called_once_with("t1", "manual")
    hl.ensure_running.assert_awaited_once_with("t1", esperar_pronta=False)
    volta.assert_called_once_with("t1")
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_troca_modo"

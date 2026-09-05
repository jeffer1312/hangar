import asyncio
import os
import pytest

from app.adapters import get_adapter
from app.adapters.omp.adapter import OmpAdapter
from app.adapters.pi.adapter import PiAdapter


def test_omp_e_registrado_e_herda_do_pi():
    a = get_adapter("omp")
    assert isinstance(a, OmpAdapter) and isinstance(a, PiAdapter)
    assert a.provider == "omp"
    for m in ("transcript_stream", "state_monitor", "drain", "send_prompt",
              "deliverable", "spawn_command", "transcript_path", "resume_command"):
        assert callable(getattr(a, m)), m


def test_spawn_usa_session_com_caminho_e_nunca_session_id(monkeypatch, tmp_path):
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path))
    cmd = OmpAdapter().spawn_command("/w", "abc", "opencode-go/deepseek-v4-flash", "low", None)
    assert "--session-id" not in cmd
    i = cmd.index("--session")
    assert cmd[i + 1].startswith(str(tmp_path / "sessions")) and cmd[i + 1].endswith("_abc.jsonl")
    assert cmd[i + 2:] == ["--model", "opencode-go/deepseek-v4-flash", "--thinking", "low"]
    esperado = [] if os.name == "nt" else ["env", "CP_PI_SESSION=abc"]
    assert cmd[:cmd.index("omp")] == esperado


def test_resume_usa_r_com_o_caminho_existente_e_o_mesmo_env(monkeypatch, tmp_path):
    from app.adapters.pi import sessions as s
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path))
    d = tmp_path / "sessions" / s.cwd_slug("/w")
    d.mkdir(parents=True)
    f = d / "2026-09-03T15-51-00-640Z_abc.jsonl"
    f.write_text("")
    cmd = OmpAdapter().resume_command("/w", "abc", None, None)
    assert cmd[cmd.index("omp"):] == ["omp", "-r", str(f)]
    assert ("CP_PI_SESSION=abc" in cmd) == (os.name != "nt")
    with pytest.raises(ValueError):
        OmpAdapter().resume_command("/w", "nao-existe", None, None)


def test_drain_e_send_prompt_passam_o_provider_da_instancia(monkeypatch):
    from app.adapters.pi import adapter as mod
    visto = {}
    monkeypatch.setattr(mod.ti, "drain", lambda name, path, provider: visto.update(drain=provider) or 0)
    monkeypatch.setattr(mod.agentpane, "pane_info", lambda name: ("omp", "%7"))

    class FakeTI:
        def send_prompt(self, name, text, provider, pane_id=None):
            visto.update(send=provider, pane=pane_id); return "sent"
    monkeypatch.setattr(mod.ti, "TerminalInput", FakeTI)
    asyncio.run(OmpAdapter().drain("s", "/x.jsonl"))
    asyncio.run(OmpAdapter().send_prompt("s", "oi"))
    assert visto == {"drain": "omp", "send": "omp", "pane": "%7"}


def test_model_args_aceita_omp_com_thinking():
    from app import model_args
    assert model_args.args_de("omp", "x/y", "max") == ["--model", "x/y", "--thinking", "max"]
    with pytest.raises(ValueError):
        model_args.args_de("omp", None, "ultracode")


def test_cli_probe_e_stats_conhecem_omp():
    from app import cli_probe, stats
    assert cli_probe._BIN["omp"] == "omp"
    assert stats._FOLDS["omp"] is stats._FoldPi


def test_argv0_omp_vira_provider_omp():
    from app.registry import _provider_do_argv
    assert _provider_do_argv(["omp"]) == "omp"
    assert _provider_do_argv(["pi"]) == "pi"

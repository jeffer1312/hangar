import asyncio
import pytest

from app.adapters import get_adapter
from app.adapters.omp.adapter import OmpAdapter
from app.adapters.pi.adapter import PiAdapter


def test_omp_e_registrado_e_herda_do_pi():
    a = get_adapter("omp")
    assert isinstance(a, OmpAdapter) and isinstance(a, PiAdapter)
    assert a.provider == "omp"
    for m in ("transcript_stream", "state_monitor", "drain", "send_prompt",
              "deliverable", "spawn_command", "transcript_path"):
        assert callable(getattr(a, m)), m


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
    from app import cli_probe
    assert cli_probe._BIN["omp"] == "omp"


def test_argv0_omp_vira_provider_omp():
    from app.registry import _provider_do_argv
    assert _provider_do_argv(["omp"]) == "omp"
    assert _provider_do_argv(["pi"]) == "pi"


def test_omp_nao_herda_o_spawn_do_pi():
    # Sessão nova do omp abre no Rust; herdar o `pi --session-id` subiria o agente errado.
    with pytest.raises(NotImplementedError):
        OmpAdapter().spawn_command("/w", "abc")

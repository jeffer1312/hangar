from types import SimpleNamespace

import pytest
from fastapi import HTTPException

from app import account_bridge, cotas, runtime_coordinator


def test_pending_does_not_fall_back_to_python(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="pending"))
    monkeypatch.setattr(account_bridge, "_preparation_transport", None)
    with pytest.raises(HTTPException) as error:
        cotas.listar_cotas()
    assert error.value.status_code == 503


def test_quota_bridge_preserves_complete_dto(monkeypatch):
    row = cotas.CotaConta(id="claude:test", label="Conta", provedor="claude", estado="lida",
                         janelas=[cotas.JanelaCota(rotulo="5h", pct=42)], ts=1000).model_dump()
    calls = []
    def request(**kwargs):
        calls.append(kwargs)
        return [row]
    monkeypatch.setattr(account_bridge, "request_quotas", request)
    assert cotas.listar_cotas(forcar=True)[0].model_dump() == row
    assert cotas.cotas_claude()[0].model_dump() == row
    assert calls == [{"force": True}, {"cached_only": True}]


def test_other_provider_facts_read_only_the_requested_sources(monkeypatch):
    sources = [cotas._Fonte("kimi:outra", "Outra", "kimi",
                           lambda: pytest.fail("fonte não pedida não é lida")),
               cotas._Fonte("kimi:test", "Kimi", "kimi",
                           lambda: ("lida", [cotas.JanelaCota(rotulo="5h", pct=25)], None))]
    monkeypatch.setattr(cotas, "_other_sources", lambda: sources)
    monkeypatch.setattr(cotas.apelidos, "ler", lambda: {"kimi:test": "Minha chave"})
    facts = cotas.quota_facts("sources", [])
    assert [source["id"] for source in facts["sources"]] == ["kimi:outra", "kimi:test"]
    assert facts["aliases"] == {"kimi:test": "Minha chave"}
    result = cotas.quota_facts("read", ["claude:test", "kimi:test"])
    assert set(result["readings"]) == {"kimi:test"}
    assert result["readings"]["kimi:test"]["janelas"][0]["pct"] == 25


def test_close_before_delayed_refresh_prevents_new_window(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from unittest.mock import Mock
    import pytest
    from app import account_bridge, runtime_coordinator, config, claude_window
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(instance="synthetic", mode="rust"))
    monkeypatch.setattr(config, "list_config_dirs", lambda: [SimpleNamespace(path=str(tmp_path))])
    monkeypatch.setattr(claude_window, "kill", Mock())
    create = Mock()
    monkeypatch.setattr(claude_window, "spawn", create)
    helper = account_bridge.ClaudeWindows()
    body = {"instance":"synthetic", "key":{"provider":"claude","canonical_home":str(tmp_path)},
            "operation":"a"*32,"action":"close","code":None}
    assert helper.run(body) == {"ok": True}
    body["action"] = "refresh"
    with pytest.raises(ValueError, match="operação encerrada"):
        helper.run(body)
    create.assert_not_called()

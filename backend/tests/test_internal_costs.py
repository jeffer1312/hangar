
import pytest

from app import costs_cache, costs_sources


def test_boot_warmup_skips_while_rust_serves(monkeypatch):
    calls = []
    monkeypatch.setattr(costs_sources, "aquecer_em_background", lambda: calls.append(1))
    costs_sources.set_served_by_rust(True)
    try:
        costs_sources._boot_warmup()
        assert calls == []
    finally:
        costs_sources.set_served_by_rust(False)
    costs_sources._boot_warmup()
    assert calls == [1]


def test_request_reaching_python_still_warms_on_demand(monkeypatch):
    """Pedido repassado ao Python dispara a varredura dele e responde com aquecimento."""
    calls = []
    monkeypatch.setattr(costs_sources, "aquecer_em_background", lambda: calls.append(1))
    monkeypatch.setattr(costs_sources._aquecido, "is_set", lambda: False)
    monkeypatch.setattr(costs_cache, "progresso_total", lambda: (0, 0))
    costs_sources.set_served_by_rust(True)
    try:
        with pytest.raises(costs_sources.Aquecendo):
            costs_sources.preparar()
    finally:
        costs_sources.set_served_by_rust(False)
    assert calls == [1]

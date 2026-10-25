"""Passos de atualização na subida do backend: no Windows, nunca os que chamam o instalador, que
reinicia as tarefas e mataria o próprio backend."""
from app import atualizacoes, main


def _chamadas(monkeypatch, pendentes):
    feitos = []
    monkeypatch.setattr(atualizacoes, "pendentes", lambda incluir_destrutivos=True: pendentes)
    monkeypatch.setattr(atualizacoes, "aplicar_pendentes", lambda incluir_destrutivos=True: feitos.append(1) or [])
    return feitos


def test_no_windows_o_instalador_pendente_espera_o_botao(monkeypatch):
    passo = {"id": "a", "comando": "powershell -ExecutionPolicy Bypass -File install.ps1 -Update"}
    feitos = _chamadas(monkeypatch, [passo])
    main._passos_pendentes_da_versao(windows=True)
    assert feitos == []


def test_fora_disso_os_passos_rodam(monkeypatch):
    passo = {"id": "a", "comando": "powershell -ExecutionPolicy Bypass -File install.ps1 -Update"}
    feitos = _chamadas(monkeypatch, [passo])
    main._passos_pendentes_da_versao(windows=False)
    assert feitos == [1]

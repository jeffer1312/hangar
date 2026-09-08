"""Gatilhos: o lançador só avisa o backend e espera um pouco; o lifespan não reconcilia sozinho."""
import asyncio
import importlib.util
import sys
from importlib.machinery import SourceFileLoader
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from app import api, codex_integracao


def _lancador():
    caminho = Path(__file__).resolve().parents[2] / "scripts" / "hangar-codex-tui"
    loader = SourceFileLoader("lancador_codex_teste", str(caminho))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    modulo = importlib.util.module_from_spec(spec)
    loader.exec_module(modulo)
    return caminho, modulo


@pytest.mark.parametrize("falha", [False, True])
def test_lancador_avisa_o_backend_antes_de_abrir_servidor_e_tolera_falha(tmp_path, monkeypatch, capsys, falha):
    caminho, lancador = _lancador()
    monkeypatch.setenv('HOME', str(tmp_path / 'home'))
    monkeypatch.setenv('CODEX_HOME', str(tmp_path / 'home/.codex'))
    ordem = []

    def api_backend(method, path):
        ordem.append((method, path))
        if falha:
            raise RuntimeError("Falha simulada")
        return {"estado": "parcial", "etapa": "Concluída", "avisos": ["Aviso simulado"],
                "erros": [], "plugins": [], "confianca_pendente": True}

    class AntesDoServidor(Exception):
        pass

    def porta():
        ordem.append("servidor")
        raise AntesDoServidor

    monkeypatch.setattr(lancador, "_api_backend", api_backend)
    monkeypatch.setattr(lancador, "_com_websockets", lambda: None)
    monkeypatch.setattr(lancador, "_porta_livre", porta)
    monkeypatch.setattr(sys, "argv", [str(caminho), "--name", "teste", "--cwd", str(tmp_path)])
    monkeypatch.setattr(sys, "path", sys.path.copy())
    with pytest.raises(AntesDoServidor):
        lancador.main()
    assert ordem == [("POST", "/api/harness/codex/integracao/sessao"), "servidor"]
    texto = capsys.readouterr().err
    if falha:
        assert "a sessão abre assim mesmo" in texto
    else:
        assert "Aviso simulado" in texto
        assert "confirmação de confiança" in texto


def test_lancador_desiste_no_prazo_e_a_reconciliacao_segue_no_backend(monkeypatch):
    _, lancador = _lancador()
    chamadas = []

    def api_backend(method, path):
        chamadas.append(method)
        return {"estado": "executando", "etapa": "Instalando", "avisos": [], "erros": []}

    monkeypatch.setattr(lancador, "_api_backend", api_backend)
    monkeypatch.setattr(lancador.time, "sleep", lambda s: None)
    resultado = lancador._integracao_codex(prazo=0.01)
    assert chamadas[0] == "POST" and "GET" in chamadas
    assert any("abre sem ela" in a for a in resultado["avisos"])


async def test_lifespan_nao_reconcilia_sozinho_e_fecha_a_integracao(tmp_path, monkeypatch):
    ordem = []

    async def fechar():
        ordem.append("fechado")

    async def nada(*args):
        pass

    servico = SimpleNamespace(fechar=fechar, iniciar=Mock(side_effect=AssertionError("não devia iniciar")))
    monkeypatch.setattr(codex_integracao, "SERVICO", servico)
    monkeypatch.setattr(api, "list_config_dirs", lambda: [])
    monkeypatch.setattr(api, "_backend_config_base", lambda: tmp_path)
    monkeypatch.setattr(api.registry, "list", lambda: [])
    monkeypatch.setattr(api.loop_mod, "_loop_dir", lambda: tmp_path)
    monkeypatch.setattr(api.hook_state, "watch", nada)
    monkeypatch.setattr(api.stall_watch, "watch", nada)
    for nome in ("_renova_token_loop", "_fetch_loop", "_auto_update_loop", "_prune_loop"):
        monkeypatch.setattr(api, nome, nada)
    monkeypatch.setattr(api.pricing, "atualizar_em_background", lambda: None)
    monkeypatch.setattr(api, "threading", SimpleNamespace(Thread=Mock(return_value=SimpleNamespace(start=lambda: None))))
    monkeypatch.setattr(api.INBOX, "ligar_loop", lambda loop: None)
    monkeypatch.setattr(api, "_loop_servidor", None)
    async with api._lifespan(api.app):
        await asyncio.sleep(0.05)
        assert ordem == []
    assert ordem == ["fechado"]

"""Gatilhos: o lançador aguarda o primeiro preparo; o lifespan não reconcilia sozinho."""
import asyncio
import importlib.util
import sys
import time
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
def test_lancador_so_abre_servidor_depois_de_preparo_confirmado(tmp_path, monkeypatch, capsys, falha):
    caminho, lancador = _lancador()
    monkeypatch.setenv('HOME', str(tmp_path / 'home'))
    monkeypatch.setenv('CODEX_HOME', str(tmp_path / 'home/.codex'))
    monkeypatch.setenv('CP_CODEX_SYNC_ENABLED', '1')
    ordem = []

    def api_backend(method, path):
        ordem.append((method, path))
        if method == "GET":
            return {"status": "idle", "issues": []}
        if falha:
            raise RuntimeError("Falha simulada")
        return {"status": "ready", "issues": []}

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
    if falha:
        assert lancador.main() == 1
    else:
        with pytest.raises(AntesDoServidor):
            lancador.main()
    from urllib.parse import quote
    path = "/api/codex-contas/default/prepare?cwd=" + quote(str(tmp_path), safe="")
    assert ordem == [("GET", path), ("POST", path)] + ([] if falha else ["servidor"])
    texto = capsys.readouterr().err
    if falha:
        assert "a conta não abriu porque o preparo não foi confirmado" in texto
        assert "Falha simulada" in texto
    else:
        assert "não abriu" not in texto


def test_lancador_espera_o_preparo_em_andamento_sem_iniciar_outro(tmp_path, monkeypatch, capsys):
    caminho, lancador = _lancador()
    monkeypatch.setenv('HOME', str(tmp_path / 'home'))
    monkeypatch.setenv('CODEX_HOME', str(tmp_path / 'home/.codex'))
    monkeypatch.setenv('CP_CODEX_SYNC_ENABLED', '1')
    chamadas = []
    estados = iter(["running", "ready"])

    def api_backend(method, path):
        chamadas.append(method)
        return {"status": next(estados), "etapa": "Instalando", "issues": []}

    class AntesDoServidor(Exception):
        pass

    def porta():
        chamadas.append("servidor")
        raise AntesDoServidor

    monkeypatch.setattr(lancador, "_api_backend", api_backend)
    monkeypatch.setattr(lancador, "_com_websockets", lambda: None)
    monkeypatch.setattr(lancador, "_porta_livre", porta)
    monkeypatch.setattr(lancador, "time", SimpleNamespace(monotonic=time.monotonic, sleep=lambda _: chamadas.append("espera")))
    monkeypatch.setattr(sys, "argv", [str(caminho), "--name", "teste", "--cwd", str(tmp_path)])
    monkeypatch.setattr(sys, "path", sys.path.copy())
    with pytest.raises(AntesDoServidor):
        lancador.main()
    assert chamadas == ["GET", "espera", "GET", "servidor"]
    assert "Instalando" in capsys.readouterr().err


async def test_lifespan_nao_reconcilia_sozinho_e_fecha_a_integracao(tmp_path, monkeypatch):
    ordem = []

    async def fechar():
        ordem.append("fechado")

    async def nada(*args):
        pass

    # `atualizar_e_aguardar` entra aqui porque o lifespan LÊ o atributo pra passar ao
    # CodexContasLogin (api.py). Ler não é chamar, e o side_effect prova isso: se a subida
    # reconciliar sozinha, o teste quebra dizendo por quê — mesma receita do `iniciar`.
    servico = SimpleNamespace(
        fechar=fechar,
        iniciar=Mock(side_effect=AssertionError("não devia iniciar")),
        atualizar_e_aguardar=Mock(side_effect=AssertionError("não devia reconciliar")),
    )
    monkeypatch.setattr(codex_integracao, "SERVICO", servico)
    monkeypatch.setattr(api, "list_config_dirs", lambda: [])
    monkeypatch.setattr(api, "_backend_config_base", lambda: tmp_path)
    monkeypatch.setattr(api.registry, "list", lambda: [])
    monkeypatch.setattr(api.loop_mod, "_loop_dir", lambda: tmp_path)
    monkeypatch.setattr(api.hook_state, "watch", nada)
    for nome in ("_fetch_loop", "_auto_update_loop", "_prune_loop"):
        monkeypatch.setattr(api, nome, nada)
    monkeypatch.setattr(api.pricing, "atualizar_em_background", lambda: None)
    monkeypatch.setattr(api, "threading", SimpleNamespace(Thread=Mock(return_value=SimpleNamespace(start=lambda: None))))
    monkeypatch.setattr(api, "_loop_servidor", None)
    async with api._lifespan(api.app):
        await asyncio.sleep(0.05)
        assert ordem == []
    assert ordem == ["fechado"]

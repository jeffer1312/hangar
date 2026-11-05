"""O lançador aguarda a confirmação do preparo nativo antes do app-server."""
import importlib.util
import sys
import time
from importlib.machinery import SourceFileLoader
from pathlib import Path
from types import SimpleNamespace

import pytest



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

"""Membro de grupo cuja sessão morreu FORA do app (Ctrl-C, crash, reboot): ninguém chama leave,
o sidecar fica fantasma. A varredura em list() cobre isso: ausente em 2 varreduras E por pelo
menos _PAIR_AUSENCIA_MIN_S (4 instâncias do registry varrem; kill/rename chamam list())."""
from unittest.mock import patch

import pytest

from app import pair, registry as registry_mod
from app.registry import SessionRegistry


@pytest.fixture(autouse=True)
def _pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(SessionRegistry, "_pair_ausencias", {})
    monkeypatch.setattr(registry_mod, "apos_saida_por_morte", None)


def _reg():
    return SessionRegistry.__new__(SessionRegistry)  # só o método; sem __init__ (tmux)


def test_ausente_por_tempo_e_dois_polls_sai_do_grupo_e_avisa_pela_fila():
    pair.join("a", "b")
    pair.join("c", "a")
    r = _reg()
    drenados = []
    registry_mod.apos_saida_por_morte = drenados.append
    with patch("app.registry.PromptQueue") as pq:
        r._varrer_pares_mortos({"b", "c"}, agora=100.0)     # 1ª ausência: só marca
        r._varrer_pares_mortos({"b", "c"}, agora=100.1)     # 2ª, mas cedo demais
        assert pair.PairLink("a").get() is not None
        pq.return_value.append.assert_not_called()
        r._varrer_pares_mortos({"b", "c"}, agora=100.0 + SessionRegistry._PAIR_AUSENCIA_MIN_S)
    assert pair.PairLink("a").get() is None
    assert pair.PairLink("b").get()["peers"] == ["c"]
    assert sorted(c.args[0] for c in pq.call_args_list) == ["b", "c"]
    for c in pq.return_value.append.call_args_list:
        assert c.args[0].startswith("[de: hangar] 'a' encerrou fora do app e saiu do grupo de trabalho.")
        assert c.kwargs.get("delivered") is False
    assert sorted(drenados) == ["b", "c"]


def test_volta_a_aparecer_zera_a_contagem():
    pair.join("a", "b")
    r = _reg()
    r._varrer_pares_mortos({"b"}, agora=100.0)
    r._varrer_pares_mortos({"a", "b"}, agora=101.0)
    r._varrer_pares_mortos({"b"}, agora=110.0)            # 1ª ausência de novo
    assert pair.PairLink("a").get() is not None


def test_contador_e_de_classe_entre_instancias():
    pair.join("a", "b")
    r1, r2 = _reg(), _reg()
    with patch("app.registry.PromptQueue"):
        r1._varrer_pares_mortos({"b"}, agora=100.0)
        r2._varrer_pares_mortos({"b"}, agora=100.0 + SessionRegistry._PAIR_AUSENCIA_MIN_S)
    assert pair.PairLink("a").get() is None                # a 2ª instância viu a 1ª ausência


def test_lista_vazia_nao_varre():
    # tmux fora do ar = lista vazia; varrer aqui dissolveria TODOS os grupos da máquina.
    pair.join("a", "b")
    r = _reg()
    r._varrer_pares_mortos(set(), agora=100.0)
    r._varrer_pares_mortos(set(), agora=200.0)
    assert pair.PairLink("a").get() is not None

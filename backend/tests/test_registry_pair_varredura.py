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
    # 2a varredura do mesmo nome ja limpo (ex: outra instancia do registry rodando o mesmo tick):
    # 'a' nao tem mais sidecar, entao nao volta a ser candidato -- mas isto so nao estoura porque a
    # remocao do dict usa pop(n, None), nunca del (achado do review de Task 8).
    with patch("app.registry.PromptQueue"):
        r._varrer_pares_mortos({"b", "c"}, agora=200.0)


class _DictRemoveNoSnapshot(dict):
    """Simula OUTRA instância removendo a MESMA chave entre o snapshot desta varredura (o `for x in
    cls._pair_ausencias` do loop de limpeza) e a remoção dela própria — a janela que só um
    `pop(n, None)` tolera (achado do review de Task 8: `del` estourava KeyError aqui)."""

    def __iter__(self):
        for k in list(dict.keys(self)):
            dict.pop(self, k, None)
            yield k


class _DictRemoveNoSetdefault(dict):
    """Mesma ideia, pro segundo ponto de remoção (pós-portão de tempo): simula a outra instância
    tendo lido o MESMO `primeira` (`setdefault` não sobrescreve) e já removido a chave um instante
    depois — antes desta instância chegar no seu próprio `pop`/`del`."""

    def setdefault(self, key, default):
        val = dict.setdefault(self, key, default)
        dict.pop(self, key, None)
        return val


def test_limpeza_de_ausencia_obsoleta_tolera_remocao_concorrente(monkeypatch):
    monkeypatch.setattr(SessionRegistry, "_pair_ausencias", _DictRemoveNoSnapshot({"fantasma": 0.0}))
    r = _reg()
    r._varrer_pares_mortos({"b"}, agora=1000.0)   # nao pode estourar KeyError
    assert "fantasma" not in SessionRegistry._pair_ausencias


def test_pos_portao_tolera_remocao_concorrente(monkeypatch):
    pair.join("a", "b")
    monkeypatch.setattr(SessionRegistry, "_pair_ausencias", _DictRemoveNoSetdefault({"a": 0.0}))
    r = _reg()
    with patch("app.registry.PromptQueue"):
        r._varrer_pares_mortos({"b"}, agora=SessionRegistry._PAIR_AUSENCIA_MIN_S)   # nao pode estourar KeyError


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


def test_nome_com_espaco_nao_e_dissolvido_por_sanitizacao():
    # sidecar de "meu proj" é "meu-proj.json" (_sanitize); vivos traz o nome CRU do tmux -- sem
    # subtrair a versão sanitizada de vivos, a sessão viva nunca sai de candidatos.
    pair.join("meu proj", "b")
    r = _reg()
    vivos = {"meu proj", "b"}
    with patch("app.registry.PromptQueue") as pq:
        r._varrer_pares_mortos(vivos, agora=100.0)
        r._varrer_pares_mortos(vivos, agora=100.0 + SessionRegistry._PAIR_AUSENCIA_MIN_S)
    assert pair.PairLink("meu proj").get() is not None
    pq.return_value.append.assert_not_called()

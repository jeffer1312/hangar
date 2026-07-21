"""Cobertura do pareamento em GRUPO (pair): join (2 soltas / merge de grupos), leave (dissolve
grupo de 1), rename (re-aponta os companheiros), formato legado 1:1 e contrato por gid. Isola o
pair dir apontando settings.projects_dir pra um tmp — mesmo padrão de test_chain.py."""
import json

import pytest

from app import pair
from app.pair import PairLink


@pytest.fixture(autouse=True)
def _tmp_pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def _peers(name):
    link = PairLink(name).get()
    return sorted(link["peers"]) if link else None


def test_join_two_loose_sessions():
    members = pair.join("a", "b", "tarefa X")
    assert sorted(members) == ["a", "b"]
    assert _peers("a") == ["b"] and _peers("b") == ["a"]
    assert PairLink("a").get()["task"] == "tarefa X"
    # gid igual dos dois lados (contrato compartilhado estável)
    assert PairLink("a").get()["gid"] == PairLink("b").get()["gid"] != ""


def test_join_merges_existing_groups():
    pair.join("a", "b")
    pair.join("c", "d")
    members = pair.join("a", "c")
    assert sorted(members) == ["a", "b", "c", "d"]
    for m in "abcd":
        assert _peers(m) == sorted(x for x in "abcd" if x != m)


def test_join_third_into_pair_keeps_gid():
    pair.join("a", "b")
    gid = PairLink("a").get()["gid"]
    pair.join("a", "c")
    assert PairLink("c").get()["gid"] == gid  # contrato não muda de arquivo


def test_leave_keeps_remaining_group():
    pair.join("a", "b")
    pair.join("a", "c")
    peers = pair.leave("a")
    assert sorted(peers) == ["b", "c"]
    assert _peers("a") is None
    assert _peers("b") == ["c"] and _peers("c") == ["b"]


def test_leave_dissolves_group_of_two():
    pair.join("a", "b")
    assert sorted(pair.leave("a")) == ["b"]
    assert _peers("a") is None and _peers("b") is None


def test_leave_unpaired_is_noop():
    assert pair.leave("solta") == []


def test_rename_repoints_companions():
    pair.join("a", "b")
    pair.join("a", "c")
    pair.rename_pair("a", "a2")
    assert _peers("a") is None
    assert _peers("a2") == ["b", "c"]
    assert _peers("b") == ["a2", "c"] and _peers("c") == ["a2", "b"]


def test_legacy_single_peer_format_is_read():
    # Sidecar antigo (1:1) gravado antes do modelo de grupo -> lido como grupo de 2.
    p = pair._pair_dir() / "velho.json"
    p.write_text(json.dumps({"peer": "outro", "task": "t"}), encoding="utf-8")
    link = PairLink("velho").get()
    assert link["peers"] == ["outro"] and link["task"] == "t"


def test_snapshot_restore_roundtrip():
    pair.join("a", "b")
    snap = pair.snapshot(["a", "c"])
    pair.join("a", "c")
    assert _peers("c") == ["a", "b"]
    pair.restore(snap)
    assert _peers("a") == ["b"] and _peers("b") == ["a"] and _peers("c") is None


def test_join_write_failure_restores_previous_state(monkeypatch):
    # Escrita parcial no meio do _write_group (ex: disco cheio) NÃO pode deixar grupo assimétrico:
    # rollback do snapshot na mesma seção crítica.
    pair.join("a", "b")
    orig = PairLink.set
    armed = {"fail": True}

    def flaky(self, peers, task="", gid=""):
        if armed["fail"]:
            armed["fail"] = False  # falha UMA vez (o restore usa set também)
            raise OSError("disco cheio")
        orig(self, peers, task, gid)

    monkeypatch.setattr(PairLink, "set", flaky)
    with pytest.raises(OSError):
        pair.join("a", "c")
    assert _peers("a") == ["b"] and _peers("b") == ["a"] and _peers("c") is None


def test_merge_appends_loser_contract(tmp_path):
    pair.join("a", "b")
    pair.join("c", "d")
    pa = pair.contract_path_for("a")
    pc = pair.contract_path_for("c")
    pa.write_text("contrato AB", encoding="utf-8")
    pc.write_text("contrato CD", encoding="utf-8")
    pair.join("a", "c")  # merge: gid de A sobrevive; conteúdo de CD é anexado
    merged = pair.contract_path_for("d")
    assert merged == pa
    body = merged.read_text(encoding="utf-8")
    assert "contrato AB" in body and "contrato CD" in body
    assert not pc.exists()


def test_contract_path_stable_and_none_without_group(tmp_path):
    assert pair.contract_path_for("solta") is None
    pair.join("a", "b")
    p1 = pair.contract_path_for("a")
    pair.join("a", "c")
    assert pair.contract_path_for("c") == p1  # gid estável: mesmo arquivo após entrar membro


def test_join_with_remote_peer_skips_its_sidecar():
    # Pareamento cross-server: peer remoto (srv::sess) entra só como STRING na lista de peers do
    # local — o sidecar dele vive na máquina dele, não aqui. Sem o skip, gravava 'srv--sess.json'
    # fantasma local e o restore/leave mexeriam num arquivo que não é desta máquina.
    members = pair.join("a", "srv::b", "tarefa cross")
    assert sorted(members) == ["a", "srv::b"]
    assert _peers("a") == ["srv::b"]          # local sabe do par remoto
    assert PairLink("srv::b").get() is None    # nenhum sidecar local pro remoto
    # nenhum arquivo com o nome sanitizado do remoto foi criado
    assert not list(pair._pair_dir().glob("srv*b.json"))


def test_leave_cross_server_pair_clears_local_only():
    pair.join("a", "srv::b")
    ex = pair.leave("a")
    assert ex == ["srv::b"]                     # devolve o par remoto pro caller avisar via /unpair-remote
    assert _peers("a") is None


def test_join_rejects_folding_local_into_cross_server_pair():
    # Achado CRÍTICO do review: 'a' pareada cross-server com srv::x; parear 'b' localmente com 'a'
    # vazava srv::x pro sidecar de 'b' sem handshake, e um unpair de 'b' dissolvia o par legítimo do
    # OUTRO server. Agora join_group rejeita a mistura na raiz.
    pair.join("a", "srv::x")
    with pytest.raises(pair.PairMixError):
        pair.join_group("b", ["a"])
    # estado intacto: 'a' segue no par cross 1:1, 'b' nunca ganhou o remoto vazado
    assert _peers("a") == ["srv::x"]
    assert _peers("b") is None


def test_join_rejects_two_remotes():
    pair.join("a", "srv::x")
    with pytest.raises(pair.PairMixError):
        pair.join_group("a", ["srv::y"])       # 2 remotos num par -> ilegal
    assert _peers("a") == ["srv::x"]           # sem mutação após o raise


def test_join_rejects_cross_pairing_a_local_group_member():
    pair.join("a", "b")                        # grupo local a<->b
    with pytest.raises(pair.PairMixError):
        pair.join_group("a", ["srv::x"])       # 'a' num grupo local não pode cross-parear
    assert _peers("a") == ["b"] and _peers("b") == ["a"]
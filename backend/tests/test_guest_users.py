# backend/tests/test_guest_users.py
import pytest

from app import guest_users

LIVES: dict[str, str | None] = {}


@pytest.fixture(autouse=True)
def _store(tmp_path, monkeypatch):
    monkeypatch.setattr(guest_users, "_path_override", tmp_path / "guests.json")
    monkeypatch.setattr(guest_users, "session_life", lambda name: LIVES.get(name))
    monkeypatch.setattr(guest_users, "_LIFE_TTL", 0.0)
    LIVES.clear()
    guest_users._reset()
    yield
    guest_users._reset()


def _guest(tmp_path, **kw):
    root = tmp_path / "proj"
    root.mkdir(exist_ok=True)
    args = {"sees_owner": False, "owner_sees": True, **kw}
    return guest_users.create("ana", str(root), **args)


def test_create_stores_only_hash_and_finds_by_token(tmp_path):
    g, token = _guest(tmp_path)
    assert token not in (tmp_path / "guests.json").read_text()
    assert guest_users.lookup_token(token).id == g.id
    assert guest_users.lookup_token("errado") is None
    assert guest_users.lookup_token("") is None


def test_create_rejects_missing_folder(tmp_path):
    with pytest.raises(guest_users.GuestError) as e:
        guest_users.create("ana", str(tmp_path / "nao-existe"), False, True)
    assert e.value.reason == "pasta_inexistente"


def test_survives_reload(tmp_path):
    g, token = _guest(tmp_path)
    guest_users._reset()
    assert guest_users.lookup_token(token).id == g.id


def test_claimed_session_belongs_to_guest(tmp_path):
    g, _ = _guest(tmp_path)
    LIVES["s1"] = "t:100"
    guest_users.claim("s1", g.id)
    assert guest_users.owner_name("s1") == "ana"


def test_same_name_new_life_is_owners(tmp_path):
    g, _ = _guest(tmp_path)
    LIVES["s1"] = "t:100"
    guest_users.claim("s1", g.id)
    LIVES["s1"] = "t:200"          # a sessão dele morreu e o dono criou outra com o mesmo nome
    assert guest_users.owner_of("s1") is None


def test_unknown_life_keeps_claim(tmp_path):
    g, _ = _guest(tmp_path)
    LIVES["s1"] = "t:100"
    guest_users.claim("s1", g.id)
    LIVES["s1"] = None              # tmux sem responder não entrega a sessão ao dono
    assert guest_users.owner_of("s1").id == g.id


def test_set_life_keeps_claim_across_mode_switch(tmp_path):
    g, _ = _guest(tmp_path)
    LIVES["s1"] = "t:100"
    guest_users.claim("s1", g.id)
    LIVES["s1"] = "k:abc"           # terminal -> sem terminal
    guest_users.set_life("s1", "k:abc")
    assert guest_users.owner_of("s1").id == g.id
    guest_users._reset()            # e sobrevive ao recarregar do disco
    assert guest_users.owner_of("s1").id == g.id


def test_set_life_ignores_unclaimed_and_unknown(tmp_path):
    g, _ = _guest(tmp_path)
    guest_users.set_life("sem-dono", "t:1")
    assert not guest_users.has_claims()
    LIVES["s1"] = "t:100"
    guest_users.claim("s1", g.id)
    guest_users.set_life("s1", None)
    assert guest_users.owner_of("s1").id == g.id


def test_visibility_rules(tmp_path):
    ana, _ = _guest(tmp_path, owner_sees=False)
    bia, _ = guest_users.create("bia", str(tmp_path / "proj"), True, True)
    LIVES.update({"a": "t:1", "b": "t:2", "dono": "t:3"})
    guest_users.claim("a", ana.id)
    guest_users.claim("b", bia.id)
    assert guest_users.visible_to(None, "dono")
    assert not guest_users.visible_to(None, "a")        # ana escondeu as dela
    assert guest_users.visible_to(None, "b")
    assert guest_users.visible_to(ana, "a")
    assert not guest_users.visible_to(ana, "dono")       # ana não vê as do dono
    assert guest_users.visible_to(bia, "dono")           # bia vê
    assert not guest_users.visible_to(bia, "a")          # convidado nunca vê outro convidado


def test_owner_fast_path_without_claims(tmp_path, monkeypatch):
    items = [{"name": "x"}, {"name": "y"}]
    monkeypatch.setattr(guest_users, "session_life", lambda n: pytest.fail("não devia consultar vida"))
    assert guest_users.filter_visible(None, items, lambda i: i["name"]) == items


def test_delete_hands_sessions_back_to_owner(tmp_path):
    g, token = _guest(tmp_path, owner_sees=False)
    LIVES["s1"] = "t:1"
    guest_users.claim("s1", g.id)
    guest_users.delete(g.id)
    assert guest_users.lookup_token(token) is None
    assert guest_users.visible_to(None, "s1")


def test_inside_root_blocks_escape(tmp_path):
    g, _ = _guest(tmp_path)
    (tmp_path / "proj" / "sub").mkdir()
    (tmp_path / "fora").mkdir()
    (tmp_path / "proj" / "atalho").symlink_to(tmp_path / "fora")
    assert guest_users.inside_root(g, str(tmp_path / "proj" / "sub"))
    assert not guest_users.inside_root(g, str(tmp_path / "proj" / ".." / "fora"))
    assert not guest_users.inside_root(g, str(tmp_path / "proj" / "atalho"))


def test_owner_of_never_raises_when_adopting_life_cannot_be_saved(tmp_path, monkeypatch):
    g, _ = _guest(tmp_path)
    guest_users.claim("s1", g.id)            # vida None: tmux ainda sem responder
    LIVES["s1"] = "t:100"

    def disk_full():
        raise OSError("disco cheio")
    monkeypatch.setattr(guest_users, "_save", disk_full)
    assert guest_users.owner_of("s1").id == g.id
    LIVES["s1"] = "t:200"                    # a vida adotada vale na memória
    assert guest_users.owner_of("s1") is None


def test_unreadable_file_is_never_overwritten(tmp_path):
    f = tmp_path / "guests.json"
    f.write_text("{corrompido")
    assert guest_users.lookup_token("qualquer") is None
    assert guest_users.visible_to(None, "s1")          # o dono segue normal
    with pytest.raises(guest_users.GuestError) as e:
        _guest(tmp_path)
    assert e.value.reason == "arquivo_ilegivel"
    assert f.read_text() == "{corrompido"


def test_a_claim_written_by_the_rust_is_seen_and_kept(tmp_path):
    """O `hangar-server` grava o dono da sessão que ele abriu direto no arquivo: a memória daqui
    o enxerga, e a próxima gravação daqui não o apaga."""
    import json
    g, _ = _guest(tmp_path)
    LIVES["rust"] = LIVES["py"] = "t:100"
    path = tmp_path / "guests.json"
    raw = json.loads(path.read_text())
    raw["sessions"]["rust"] = {"guest": g.id, "life": "t:100"}
    path.write_text(json.dumps(raw, indent=4))
    assert guest_users.owner_name("rust") == "ana"
    guest_users.claim("py", g.id)
    assert set(json.loads(path.read_text())["sessions"]) == {"rust", "py"}


def test_python_writes_serialize_on_the_guests_lock(tmp_path):
    import threading
    from app import atomico
    g, _ = _guest(tmp_path)
    LIVES["s1"] = "t:100"
    done = threading.Event()
    with atomico.locked(tmp_path / "guests.lock"):
        writer = threading.Thread(target=lambda: (guest_users.claim("s1", g.id), done.set()))
        writer.start()
        assert not done.wait(0.2), "gravou com a trava presa"
    assert done.wait(30)
    writer.join()
    assert guest_users.owner_name("s1") == "ana"


def test_a_read_that_fails_mid_swap_is_retried_on_the_next_call(tmp_path, monkeypatch):
    """No Windows a leitura no meio da troca do Rust falha: ela passa pelo `atomico.read` e a
    próxima chamada tenta de novo, sem esperar outra gravação."""
    from app import atomico
    g, _ = _guest(tmp_path)
    LIVES["s1"] = "t:100"
    guest_users.claim("s1", g.id)
    guest_users._reset()
    real, calls = atomico.read, []

    def flaky(path):
        calls.append(path)
        if len(calls) == 1:
            raise PermissionError("em uso")
        return real(path)
    monkeypatch.setattr(atomico, "read", flaky)
    assert [guest_users.owner_name("s1"), guest_users.owner_name("s1")] == [None, "ana"]

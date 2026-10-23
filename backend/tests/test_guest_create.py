# backend/tests/test_guest_create.py
import pytest

from app import fs, guest_users


@pytest.fixture
def ana(tmp_path, monkeypatch):
    monkeypatch.setattr(guest_users, "_path_override", tmp_path / "guests.json")
    guest_users._reset()
    (tmp_path / "p" / "sub").mkdir(parents=True)
    g, _ = guest_users.create("ana", str(tmp_path / "p"), False, True)
    marker = guest_users.current.set(g)
    yield g, tmp_path
    guest_users.current.reset(marker)
    guest_users._reset()


def test_guest_roots_are_only_his_folder(ana):
    g, _ = ana
    assert [str(r.path) for r in fs.list_roots()] == [g.root]


def test_guest_cannot_scan_owner_root(ana, monkeypatch):
    _, tmp = ana
    (tmp / "outra").mkdir()
    with pytest.raises(fs.FsError):
        fs.scan_dir(str(tmp / "outra"))


def test_guest_scans_inside_his_folder(ana):
    g, _ = ana
    assert fs.scan_dir(g.root) is not None


@pytest.fixture
def guest_shortcut(ana, monkeypatch):
    from fastapi.testclient import TestClient
    from app import api, shortcut_terminals
    g, tmp = ana
    monkeypatch.setattr(api.settings, "auth_token", "secret")
    monkeypatch.setattr(guest_users, "session_life", lambda n: "t:1")
    _, tok = guest_users.create("bia", g.root, False, True)
    bia = guest_users.lookup_token(tok)
    guest_users.claim("cc", bia.id)
    monkeypatch.setattr(api, "_session_cwd", lambda n: g.root)
    started = []
    monkeypatch.setattr(shortcut_terminals, "start", lambda *a, **k: started.append(a) or None)
    monkeypatch.setattr(shortcut_terminals, "start_hangar", lambda *a, **k: started.append(a) or (None, False))
    client = TestClient(api.app)
    return lambda **body: client.post("/api/sessions/cc/shortcut-shell",
                                      headers={"Authorization": f"Bearer {tok}"},
                                      json={"command": "ls", **body}), started, tmp


def test_guest_user_cannot_run_hangar_shortcut(guest_shortcut):
    post, started, _ = guest_shortcut
    r = post(runs_in="hangar", key="global:k")
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_shortcut_hangar_convidado"
    assert started == []


def test_guest_user_shortcut_folder_must_be_inside_his_root(guest_shortcut):
    post, started, tmp = guest_shortcut
    r = post(pasta=str(tmp))
    assert r.status_code == 403
    assert r.json()["detail"]["code"] == "erro_fora_da_pasta"
    assert started == []


def test_transfer_result_survives_guest_life_failure(ana, monkeypatch):
    import asyncio
    from app import api

    async def troca():
        return {"ok": True}

    def boom(session, life):
        raise OSError("disco cheio")
    monkeypatch.setattr(api, "session_life", lambda n: "k:novo")
    monkeypatch.setattr(api.share_bridge, "set_life", lambda n, life: None)
    monkeypatch.setattr(api.share_bridge, "mode_change", lambda n, on: None)
    monkeypatch.setattr(guest_users, "set_life", boom)
    assert asyncio.run(api._durante_troca("proj", troca())) == {"ok": True}

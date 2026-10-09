"""Rotas internas que o Rust usa nas operações de grupo: texto do protocolo e fatos da orquestração."""
import pytest
from fastapi.testclient import TestClient

import app.api as api_mod
from app import internal_api, orq_context as oc, orq_md, orq_papeis as roles, pair, pair_texto
from app.adapters.orq import runs

SECRET = "cd" * 32
H = {"X-Hangar-Internal": SECRET}


@pytest.fixture(autouse=True)
def _env(tmp_path, monkeypatch):
    internal_api.set_secret(SECRET)
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(pair, "_pair_dir", lambda: tmp_path)
    monkeypatch.setattr(pair, "_arquivo_dir", lambda: tmp_path / "archive")
    monkeypatch.setattr(oc, "identity", lambda name: "identity:" + name)
    monkeypatch.setattr(oc, "active_gid", lambda name: None)
    monkeypatch.setattr(runs, "group_phase", lambda gid: None)
    yield
    internal_api.set_secret(None)


def _post(path, body, headers=H):
    return TestClient(api_mod.app, client=("127.0.0.1", 50000)).post(f"/internal/{path}", json=body, headers=headers)


def _text(**kw):
    body = {"kind": "group", "me": "a", "others": ["b"], "task": "t", "contract": "/c.md", "contract_remote": False,
            "harness": {"a": "codex"}, "peer": "", "owner": ""}
    return _post("pair/text", {**body, **kw})


def test_routes_need_the_secret():
    for path in ("pair/text", "orq/group-phase", "orq/promote", "orq/is-orchestrator", "orq/associate"):
        assert _post(path, {}, headers={}).status_code == 404, path


def test_protocol_text_comes_from_pair_texto():
    assert _text().json() == {"text": pair_texto.texto_grupo("a", ["b"], "t", "/c.md", {"a": "codex"})}
    assert _text(contract=None).json() == {"text": pair_texto.texto_grupo("a", ["b"], "t", None, {"a": "codex"})}
    assert _text(kind="orq").json() == {"text": pair_texto.texto_grupo_orq("t")}
    assert _text(kind="external", peer="p", owner="o").json() == {"text": pair_texto.texto_par_externo("a", "p", "o")}


def test_protocol_text_refuses_bad_bodies():
    assert _text(kind="outro").status_code == 422
    assert _text(extra=1).status_code == 422
    assert _text(others=[]).status_code == 400


def test_group_phase(monkeypatch):
    for phase in ("live", "ended", "unknown", None):
        monkeypatch.setattr(runs, "group_phase", lambda gid, phase=phase: phase if gid == "g1" else "errado")
        assert _post("orq/group-phase", {"gid": "g1"}).json() == {"phase": phase}


def test_promote_maps_conflicts_and_tolerates_missing_identity(monkeypatch):
    calls = []
    monkeypatch.setattr(oc, "promote", lambda name, gid: calls.append((name, gid)))
    r = _post("orq/promote", {"name": "a", "gid": "g1"})
    assert (r.status_code, r.json(), calls) == (200, {}, [("a", "g1")])

    def raising(exc):
        def promote(name, gid):
            raise exc
        return promote

    # Como no `pair.join_group`: sem identidade não houve configuração para promover.
    monkeypatch.setattr(oc, "promote", raising(oc.IdentityUnavailable("sem identidade")))
    assert _post("orq/promote", {"name": "a", "gid": "g1"}).status_code == 200
    for exc in (oc.PromotionConflict("o time já pertence a outro grupo"), orq_md.Conflito("/x.md")):
        monkeypatch.setattr(oc, "promote", raising(exc))
        r = _post("orq/promote", {"name": "a", "gid": "g1"})
        assert r.status_code == 409
        assert r.json()["detail"] == {"code": "erro_orq_arquivo_mudou", "params": {}, "msg": str(exc)}


def test_promote_with_real_context_moves_the_draft():
    context = oc.resolve("a")
    text = roles.escrever_papel("", roles.Papel("executor", "x*", "codex", "c", "", ""))
    oc.write(context, text, 0.0)
    saved = context.path.read_text()
    assert _post("orq/promote", {"name": "a", "gid": "realgid"}).status_code == 200
    assert roles.regras_path("realgid").read_text() == saved
    r = _post("orq/promote", {"name": "b", "gid": "draft-x"})
    assert (r.status_code, r.json()["detail"]["code"]) == (409, "erro_orq_arquivo_mudou")


def test_is_orchestrator(monkeypatch):
    monkeypatch.setattr(api_mod.orq_runs, "find", lambda name: {"name": name} if name.endswith("-orq") else None)
    r = _post("orq/is-orchestrator", {"names": ["a", "g1-orq", "b"]})
    assert r.json() == {"names": ["g1-orq"]}


class _NoLock:
    def __enter__(self):
        raise AssertionError("a rota interna não pode tomar o pair._LOCK: quem segura o lock é o Rust")

    def __exit__(self, *exc):
        return False


def _saved(name):
    context = oc.resolve(name)
    text = roles.escrever_papel("", roles.Papel("executor", "x*", "codex", "c", "", ""))
    return oc.write(context, text, 0.0)


def test_associate_runs_without_pair_lock(monkeypatch):
    mtime = _saved("planner")
    pair.PairLink("arbiter").set([], "obra", "real", orq=True)
    monkeypatch.setattr(pair, "_LOCK", _NoLock())
    r = _post("orq/associate", {"name": "planner", "gid": "real", "mtime": mtime})
    assert r.status_code == 200, r.text
    body = r.json()
    assert (body["ok"], body["gid"], body["grouped"], body["session_prefix"]) == (True, "real", True, "planner")
    assert body["arquivo"] == str(roles.regras_path("real")) and body["mtime"] > 0


def test_associate_errors_match_the_public_route():
    mtime = _saved("planner")
    r = _post("orq/associate", {"name": "planner", "gid": "missing", "mtime": mtime})
    assert (r.status_code, r.json()["detail"]["code"]) == (409, "erro_orq_celula_invalida")
    pair.PairLink("arbiter").set([], "obra", "real", orq=True)
    r = _post("orq/associate", {"name": "planner", "gid": "real", "mtime": 0.0})
    assert (r.status_code, r.json()["detail"]["code"]) == (409, "erro_orq_arquivo_mudou")


def test_public_associate_still_takes_the_lock(monkeypatch):
    mtime = _saved("planner")
    pair.PairLink("arbiter").set([], "obra", "real", orq=True)
    monkeypatch.setattr(pair, "_LOCK", _NoLock())
    with pytest.raises(AssertionError, match="pair._LOCK"):
        oc.associate("planner", "real", mtime)

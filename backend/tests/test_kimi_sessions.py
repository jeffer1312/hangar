import json

from app.adapters.kimi import sessions as ks


def test_workdir_key_matches_measured_layout():
    # Medido no Kimi 0.34.0: wd_<basename>_<sha256(cwd)[:12]>.
    # /home/jefferson/Projetos/hangar -> wd_hangar_5112ff7a84e0 (dir real em ~/.kimi-code/sessions)
    assert ks.workdir_key("/home/jefferson/Projetos/hangar") == "wd_hangar_5112ff7a84e0"
    assert ks.workdir_key("/tmp/kimi-acp-probe") == "wd_kimi-acp-probe_15ca61fc9ec9"


def _slug(cwd: str) -> str:
    return ks.workdir_key(cwd).split("_")[1]


def test_workdir_key_slug_segue_o_slugify_do_cli():
    # Regra lida do binario do CLI 0.36.1 (slugifyWorkDirName) e conferida contra as chaves que ele
    # mesmo escreveu em ~/.kimi-code/workspace-trust: minusculas, tudo fora de [a-z0-9._-] vira "-",
    # hifen das pontas cai. Sem o minusculas, pasta com maiuscula no nome ganhava um pre-trust que o
    # CLI nunca achava e a TUI abria no "Trust this folder?".
    assert _slug("/tmp/MinhaPasta") == "minhapasta"
    assert _slug("/tmp/Área de trabalho") == "rea-de-trabalho"
    # Casos de borda: vazio vira "workspace", e corta em 40 chars.
    assert _slug("/@@@") == "workspace"
    assert _slug("/" + "a" * 60) == "a" * 40
    # Corte caindo EM CIMA de um hifen: o CLI faz o strip das pontas duas vezes (antes e depois do
    # slice), entao o "-" que sobra na ponta cai e o slug tem 39 chars, nao 40 terminando em "-".
    assert _slug("/" + "a" * 39 + "-" + "b" * 10) == "a" * 39


def test_transcript_path_via_session_index(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    sdir = tmp_path / "sessions" / "wd_x_abcd" / "session_11111111-2222-3333-4444-555555555555"
    wire = sdir / "agents" / "main" / "wire.jsonl"
    wire.parent.mkdir(parents=True)
    wire.write_text("")
    (tmp_path / "session_index.jsonl").write_text(
        json.dumps({"sessionId": "session_11111111-2222-3333-4444-555555555555",
                    "sessionDir": str(sdir), "workDir": "/x"}) + "\n", encoding="utf-8")
    assert ks.transcript_path("/x", "session_11111111-2222-3333-4444-555555555555") == str(wire)


def test_transcript_path_fallback_computed_key(tmp_path, monkeypatch):
    # Sem entrada no indice (janela entre o bilhete do hook e o flush do session_index): cai na
    # chave computada. Sem o DIRETORIO da sessao -> "" (sessao ainda nao existe, nao e erro).
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    sid = "session_aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    assert ks.transcript_path("/w", sid) == ""
    sdir = tmp_path / "sessions" / ks.workdir_key("/w") / sid
    sdir.mkdir(parents=True)
    assert ks.transcript_path("/w", sid) == str(sdir / "agents" / "main" / "wire.jsonl")


def test_is_subagent_wire_and_root():
    main = "/h/sessions/wd_x_y/session_123/agents/main/wire.jsonl"
    sub = "/h/sessions/wd_x_y/session_123/agents/agent-0/wire.jsonl"
    assert not ks.is_subagent_wire(main)
    assert ks.is_subagent_wire(sub)
    assert ks.root_wire(sub) == "/h/sessions/wd_x_y/session_123/agents/main/wire.jsonl"
    assert ks.root_wire(main) == ""


def test_pretrust_writes_workspace_trust_once(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    ks.pretrust_cwd("/tmp/kimi-acp-probe")
    f = tmp_path / "workspace-trust" / "wd_kimi-acp-probe_15ca61fc9ec9"
    assert f.is_file()
    data = json.loads(f.read_text())
    assert data["root"] == "/tmp/kimi-acp-probe"
    assert isinstance(data["trustedAt"], int)
    # Segunda chamada NAO reescreve (preserva o trustedAt original do CLI).
    f.write_text('{"root":"/tmp/kimi-acp-probe","trustedAt":1}')
    ks.pretrust_cwd("/tmp/kimi-acp-probe")
    assert json.loads(f.read_text())["trustedAt"] == 1


# --- Bilhete pane->sessao com JSON valido do TIPO errado ---
# `json.loads` aceita `null`, lista e string, e o `.get` num nao-dict e AttributeError — que NAO e
# nem OSError nem ValueError, entao o except das duas funcoes deixava subir. Elas rodam dentro do
# loop de `SessionRegistry.list()`, que nao tem guarda: um bilhete torto apagava TODAS as sessoes
# da tela (Claude, Codex e Pi junto), nao so a Kimi. Bilhete torto e cenario real — o _write_marker
# do hook usava tmp de nome fixo, entao dois eventos sobrepostos entrelacavam bytes.
import json as _json

import pytest as _pytest

from app import registry as _registry


@_pytest.mark.parametrize("lixo", ["null", "[1, 2, 3]", '"uma string"', "42"])
def test_bilhete_nao_dict_devolve_none_em_vez_de_estourar(monkeypatch, tmp_path, lixo):
    for sub, fn in ((".claude-pocket-kimi", _registry.kimi_session_file),
                    (".claude-pocket-pi", _registry.pi_session_file)):
        cfg = tmp_path / sub.lstrip(".")
        (cfg / sub).mkdir(parents=True, exist_ok=True)
        (cfg / sub / "123.json").write_text(lixo, encoding="utf-8")
        monkeypatch.setattr(_registry, "_config_dir_of", lambda pid, _c=cfg: _c)
        assert fn("%123", pid=7, cwd="/w") is None


def test_bilhete_dict_normal_ainda_e_lido(monkeypatch, tmp_path):
    # Contra-prova: a guarda de tipo nao pode recusar bilhete legitimo. Sem ts confiavel o frescor
    # reprova (devolve None), entao o que este teste garante e que o caminho NAO estoura.
    cfg = tmp_path / "cfg"
    (cfg / ".claude-pocket-kimi").mkdir(parents=True)
    (cfg / ".claude-pocket-kimi" / "123.json").write_text(
        _json.dumps({"session_id": "session_abc", "cwd": "/w", "ts": 1.0}), encoding="utf-8")
    monkeypatch.setattr(_registry, "_config_dir_of", lambda pid: cfg)
    assert _registry.kimi_session_file("%123", pid=7, cwd="/w") is None

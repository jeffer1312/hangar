import json
import os
from pathlib import Path

import pytest

from app.adapters.kimi import sessions as ks


@pytest.mark.skipif(os.name != "posix",
                    reason="a chave e sha256 do caminho ABSOLUTO, e estes vetores foram medidos "
                           "num Kimi real com caminho POSIX; no Windows o abspath reancora "
                           "'/home/...' em 'C:\\home\\...' e o hash muda, como tem de mudar")
def test_workdir_key_matches_measured_layout():
    # Medido no Kimi 0.34.0: wd_<basename>_<sha256(cwd)[:12]>.
    # /home/jefferson/Projetos/hangar -> wd_hangar_5112ff7a84e0 (dir real em ~/.kimi-code/sessions)
    assert ks.workdir_key("/home/jefferson/Projetos/hangar") == "wd_hangar_5112ff7a84e0"
    assert ks.workdir_key("/tmp/kimi-acp-probe") == "wd_kimi-acp-probe_15ca61fc9ec9"


def test_workdir_key_e_estavel_e_separa_pastas_diferentes():
    """A PROPRIEDADE, que vale nos dois sistemas — o vetor medido acima so vale no POSIX.

    Sem este caso, o Windows ficaria sem cobertura nenhuma de workdir_key: o unico teste dela
    dependia de um hash medido num caminho que o Windows nem consegue formar.
    """
    a = os.path.abspath(os.path.join(os.sep, "w", "projeto-a"))
    b = os.path.abspath(os.path.join(os.sep, "w", "projeto-b"))
    assert ks.workdir_key(a) == ks.workdir_key(a)          # estavel
    assert ks.workdir_key(a) != ks.workdir_key(b)          # separa
    assert ks.workdir_key(a).startswith("wd_projeto-a_")   # basename no rotulo


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
    # O esperado vem de `Path`, nao de uma string com `/` na mao: `root_wire` devolve caminho
    # NATIVO (`str(Path(...))`), entao no Windows ele sai com `\`. No POSIX esta linha e
    # byte-identica a de antes — o caso continua valendo nos dois em vez de virar skip.
    assert ks.root_wire(sub) == str(Path(main))
    assert ks.root_wire(main) == ""


def test_pretrust_writes_workspace_trust_once(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    # A chave sai do proprio `workdir_key` em vez do hash medido chumbado: aquele valor so existe
    # pro caminho POSIX, e o assunto DESTE caso e "escreve uma vez e nao reescreve", nao qual e o
    # hash — quem trava o hash e o caso medido acima. `abspath` porque e o que o pretrust usa.
    alvo = os.path.abspath(os.path.join(os.sep, "tmp", "kimi-acp-probe"))
    ks.pretrust_cwd(alvo)
    f = tmp_path / "workspace-trust" / ks.workdir_key(alvo)
    assert f.is_file()
    data = json.loads(f.read_text())
    assert data["root"] == alvo
    assert isinstance(data["trustedAt"], int)
    # Segunda chamada NAO reescreve (preserva o trustedAt original do CLI).
    f.write_text(json.dumps({"root": alvo, "trustedAt": 1}), encoding="utf-8")
    ks.pretrust_cwd(alvo)
    assert json.loads(f.read_text(encoding="utf-8"))["trustedAt"] == 1


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
    for sub, fn in ((".hangar-kimi", _registry.kimi_session_file),
                    (".hangar-pi", _registry.pi_session_file)):
        cfg = tmp_path / sub.lstrip(".")
        (cfg / sub).mkdir(parents=True, exist_ok=True)
        (cfg / sub / "123.json").write_text(lixo, encoding="utf-8")
        monkeypatch.setattr(_registry, "_config_dir_of", lambda pid, _c=cfg: _c)
        assert fn("%123", pid=7, cwd="/w") is None


def test_bilhete_dict_normal_ainda_e_lido(monkeypatch, tmp_path):
    # Contra-prova: a guarda de tipo nao pode recusar bilhete legitimo. Sem ts confiavel o frescor
    # reprova (devolve None), entao o que este teste garante e que o caminho NAO estoura.
    cfg = tmp_path / "cfg"
    (cfg / ".hangar-kimi").mkdir(parents=True)
    (cfg / ".hangar-kimi" / "123.json").write_text(
        _json.dumps({"session_id": "session_abc", "cwd": "/w", "ts": 1.0}), encoding="utf-8")
    monkeypatch.setattr(_registry, "_config_dir_of", lambda pid: cfg)
    assert _registry.kimi_session_file("%123", pid=7, cwd="/w") is None


# --- Chave do bilhete: no psmux o %pane_id NAO e unico ----------------------------------------
# Medido em 21/08/2026 na VM Windows: quatro sessoes vivas ao mesmo tempo, TODAS com
# `TMUX_PANE=%1` (o psmux numera pane por sessao, o tmux numera por servidor). Com o pane como
# chave, dois panes Kimi dividiriam UM bilhete e o backend resolveria os dois pro mesmo wire — uma
# sessao abrindo a conversa da outra. A chave passou a ser `PSMUX_SESSION` quando ela existe.
#
# Estes dois casos alimentam o HOOK de verdade (subprocesso, payload no stdin) e leem o disco,
# entao nao dependem de ter o Kimi instalado. Antes da correcao o primeiro falharia por gravar um
# bilhete so; ele nao existia, e essa era a lacuna de cobertura que deixou o bug passar.


def _roda_hook(payload: dict, env_extra: dict) -> None:
    import subprocess
    import sys as _sys
    hook = Path(__file__).resolve().parents[1] / "hooks" / "kimi_state_hook.py"
    env = {**os.environ, **env_extra}
    env.pop("PSMUX_SESSION", None)
    env.update(env_extra)          # env_extra manda, mesmo pra apagar
    subprocess.run([_sys.executable, str(hook)],
                   input=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
                   capture_output=True, env=env)


def test_dois_panes_do_psmux_geram_bilhetes_DISTINTOS(tmp_path):
    """No psmux os dois panes sao `%1`; o que os separa e o PSMUX_SESSION."""
    for sessao, sid in (("alfa", "session_aaa"), ("beta", "session_bbb")):
        _roda_hook({"hook_event_name": "SessionStart", "session_id": sid, "cwd": "/w"},
                   {"CLAUDE_CONFIG_DIR": str(tmp_path), "TMUX_PANE": "%1",
                    "PSMUX_SESSION": sessao})

    d = tmp_path / ".hangar-kimi"
    nomes = sorted(p.name for p in d.glob("*.json"))
    assert nomes == ["alfa.json", "beta.json"], "um bilhete por SESSAO, nao um so por '%1'"
    assert json.loads((d / "alfa.json").read_text(encoding="utf-8"))["session_id"] == "session_aaa"
    assert json.loads((d / "beta.json").read_text(encoding="utf-8"))["session_id"] == "session_bbb"


def test_sem_psmux_a_chave_continua_sendo_o_pane(tmp_path):
    """Contra-prova do ramo POSIX: sem PSMUX_SESSION a chave e o %N, como sempre foi."""
    _roda_hook({"hook_event_name": "SessionStart", "session_id": "session_ccc", "cwd": "/w"},
               {"CLAUDE_CONFIG_DIR": str(tmp_path), "TMUX_PANE": "%7"})
    d = tmp_path / ".hangar-kimi"
    assert [p.name for p in d.glob("*.json")] == ["7.json"]


def test_backend_resolve_cada_pane_pro_bilhete_da_sua_sessao(monkeypatch, tmp_path):
    """A outra ponta: com o mesmo `%1` nos dois, o backend tem de separar pelo PSMUX_SESSION."""
    d = tmp_path / ".hangar-kimi"
    d.mkdir(parents=True)
    (d / "alfa.json").write_text(json.dumps({"session_id": "session_aaa", "cwd": "/w", "ts": 1.0}),
                                 encoding="utf-8")
    (d / "beta.json").write_text(json.dumps({"session_id": "session_bbb", "cwd": "/w", "ts": 1.0}),
                                 encoding="utf-8")
    monkeypatch.setattr(_registry, "_config_dir_of", lambda pid: tmp_path)
    # Cada pid "vive" numa sessao psmux diferente, com o MESMO pane id.
    por_pid = {11: "alfa", 12: "beta"}
    monkeypatch.setattr(_registry.procinfo, "_env_var_of",
                        lambda pid, nome: por_pid.get(pid) if nome == "PSMUX_SESSION" else None)
    assert _registry._chave_do_bilhete("%1", 11) == "alfa"
    assert _registry._chave_do_bilhete("%1", 12) == "beta"
    # E sem a variavel (POSIX) volta a ser o pane.
    monkeypatch.setattr(_registry.procinfo, "_env_var_of", lambda pid, nome: None)
    assert _registry._chave_do_bilhete("%9", 11) == "9"

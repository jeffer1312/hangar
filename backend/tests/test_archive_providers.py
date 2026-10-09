"""Conversa morta de Pi, Kimi e Codex: cada um guarda de um jeito e nenhum guarda como o Claude.

Os fixtures montam o layout REAL de cada um (medido no disco), porque e justamente o layout que o
codigo tem que acertar -- um fake "como a doc sugere" passaria sem provar nada.
"""
import json
from pathlib import Path

import pytest

from app import archive_providers as ap
from app import codex_contas
import codex_contas_apoio


PI_SID = "602c251a-5233-42be-9b55-585c88e072f2"
OMP_SID = "7c9f9a10-3b44-4b77-9c2e-9a2c9e6f2b31"
KIMI_SID = "session_b943c017-8097-4616-984d-9dfba4a3b1e8"
CODEX_SID = "019f99de-9572-7221-89c0-80c62f883d44"


@pytest.fixture
def pi_home(tmp_path, monkeypatch):
    from app.adapters.pi import sessions as pi_sessions
    raiz = tmp_path / "pi"
    # O nome do dir e o slug do cwd; o do arquivo leva timestamp NA FRENTE do uuid.
    d = raiz / "--home-u-proj--"
    d.mkdir(parents=True)
    j = d / f"2026-08-05T09-09-13-040Z_{PI_SID}.jsonl"
    j.write_text("\n".join([
        json.dumps({"type": "session", "version": 3, "id": PI_SID, "cwd": "/home/u/proj"}),
        json.dumps({"type": "message", "id": "m1", "message": {
            "role": "assistant", "content": [{"type": "text", "text": "resposta do pi"}]}}),
    ]) + "\n", encoding="utf-8")
    # Subagente: mora em <stem>/<taskId>/run-N/session.jsonl e NAO e conversa.
    sub = d / f"2026-08-05T09-09-13-040Z_{PI_SID}" / "44bad0fb" / "run-2"
    sub.mkdir(parents=True)
    (sub / "session.jsonl").write_text("{}\n", encoding="utf-8")
    # Provider "omp" cai numa pasta que não existe: sem isto, _LISTAR["omp"] leria a MESMA raiz
    # e devolveria a conversa do Pi de novo, só que com provider="omp" (Conversa usa o provider
    # PEDIDO, não deriva do path).
    monkeypatch.setattr(pi_sessions, "sessions_root",
                         lambda provider: raiz if provider == "pi" else tmp_path / "sem-omp")
    return j


@pytest.fixture
def omp_home(tmp_path, monkeypatch):
    # Mesmo layout do pi_home, em raiz PRÓPRIA (arvore diferente da do pi), pra provar que o
    # provider "omp" resolve sozinho e não se mistura com o do Pi.
    from app.adapters.pi import sessions as pi_sessions
    raiz_pi_dir = tmp_path / "pi"
    raiz_omp_dir = tmp_path / "omp"
    d = raiz_omp_dir / "--home-u-proj--"
    d.mkdir(parents=True)
    j = d / f"2026-08-05T09-09-13-040Z_{OMP_SID}.jsonl"
    j.write_text("\n".join([
        json.dumps({"type": "session", "version": 3, "id": OMP_SID, "cwd": "/home/u/proj"}),
        json.dumps({"type": "message", "id": "m1", "message": {
            "role": "assistant", "content": [{"type": "text", "text": "resposta do omp"}]}}),
    ]) + "\n", encoding="utf-8")
    # Subagente do omp: `<stem>/<Nome>.jsonl` direto (sem run-N do Pi) — o glob raso da conversa
    # já é por diretório, então nem precisa saber o formato exato pra ignorá-lo.
    sub = d / f"2026-08-05T09-09-13-040Z_{OMP_SID}"
    sub.mkdir(parents=True)
    (sub / "Explorer.jsonl").write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(pi_sessions, "sessions_root",
                         lambda provider: raiz_pi_dir if provider == "pi" else raiz_omp_dir)
    return j


@pytest.fixture
def kimi_home(tmp_path, monkeypatch):
    from app.adapters.kimi import sessions as kimi_sessions
    home = tmp_path / "kimi"
    sdir = home / "sessions" / "wd_proj_79d86abc032b" / KIMI_SID
    wire = sdir / "agents" / "main" / "wire.jsonl"
    wire.parent.mkdir(parents=True)
    wire.write_text("{}\n", encoding="utf-8")
    (home / "session_index.jsonl").write_text(json.dumps({
        "sessionId": KIMI_SID, "sessionDir": str(sdir), "workDir": "/home/u/proj",
    }) + "\n", encoding="utf-8")
    monkeypatch.setattr(kimi_sessions, "kimi_home", lambda: home)
    return wire


@pytest.fixture
def codex_home(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    home = tmp_path / "codex"
    d = home / "sessions" / "2026" / "07" / "25"
    d.mkdir(parents=True)
    j = d / f"rollout-2026-07-25T12-22-09-{CODEX_SID}.jsonl"
    j.write_text(json.dumps({
        "timestamp": "2026-07-25T15:23:08.027Z", "type": "session_meta",
        "payload": {"session_id": CODEX_SID, "cwd": "/home/u/proj"},
    }) + "\n", encoding="utf-8")
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", home)
    return j


def test_pi_lista_conversa_e_ignora_subagente(pi_home):
    convs = ap._pi_conversas()
    assert [(c.provider, c.session_id, c.cwd) for c in convs] == [("pi", PI_SID, "/home/u/proj")]
    # O caminho sai do session_id por BUSCA: o timestamp do nome nao da pra recriar.
    assert ap.jsonl_de("pi", PI_SID) == pi_home


def test_omp_lista_conversa_com_provider_omp_e_ignora_subagente(omp_home):
    convs = ap._pi_conversas("omp")
    assert [(c.provider, c.session_id, c.cwd) for c in convs] == [("omp", OMP_SID, "/home/u/proj")]
    assert ap.jsonl_de("omp", OMP_SID) == omp_home


def test_kimi_le_o_indice_sem_abrir_transcript(kimi_home):
    convs = ap._kimi_conversas()
    assert [(c.session_id, c.cwd) for c in convs] == [(KIMI_SID, "/home/u/proj")]
    assert ap.jsonl_de("kimi", KIMI_SID) == kimi_home


def test_codex_le_o_cwd_do_session_meta(codex_home):
    convs = ap._codex_conversas()
    assert [(c.session_id, c.cwd) for c in convs] == [(CODEX_SID, "/home/u/proj")]
    assert ap.jsonl_de("codex", CODEX_SID) == codex_home


def test_session_id_de_outro_provider_e_recusado(pi_home, kimi_home):
    # O sid do Kimi (`session_<uuid>`) nao e uuid: aceitar aqui viraria glob com texto do cliente.
    with pytest.raises(ValueError):
        ap.jsonl_de("pi", KIMI_SID)
    with pytest.raises(ValueError):
        ap.jsonl_de("kimi", PI_SID)
    with pytest.raises(ValueError):
        ap.jsonl_de("nao-existe", PI_SID)
    with pytest.raises(FileNotFoundError):
        ap.jsonl_de("pi", "00000000-0000-0000-0000-000000000000")


def test_provider_quebrado_nao_derruba_os_outros(pi_home, kimi_home, codex_home, monkeypatch):
    # Layout de um provider muda / home some: ele sai da lista, os outros continuam. Sem isto o
    # Arquivo inteiro (Claude incluso) morreria por causa de um agente que a pessoa nem usa.
    def explode():
        raise RuntimeError("layout mudou")
    monkeypatch.setitem(ap._LISTAR, "pi", explode)
    provedores = {c.provider for c in ap.conversas()}
    assert provedores == {"kimi", "codex"}


def test_kimi_sem_wire_fica_de_fora(tmp_path, monkeypatch):
    # Sessao aberta e nunca usada: a TUI so cria o wire no 1o prompt. Listar sem transcript daria
    # uma linha que nao abre.
    from app.adapters.kimi import sessions as kimi_sessions
    home = tmp_path / "kimi"
    home.mkdir()
    (home / "session_index.jsonl").write_text(json.dumps({
        "sessionId": KIMI_SID, "sessionDir": str(home / "sessions" / "x" / KIMI_SID),
        "workDir": "/home/u/proj",
    }) + "\n", encoding="utf-8")
    monkeypatch.setattr(kimi_sessions, "kimi_home", lambda: home)
    assert ap._kimi_conversas() == []


@pytest.fixture
def duas_contas_codex(tmp_path, monkeypatch):
    monkeypatch.setattr(__import__("pathlib").Path, "home",
                        classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", tmp_path / ".codex")
    default = codex_contas.Account("default", tmp_path / ".codex", True)
    work = codex_contas_apoio.create_account("work")
    sid = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
    paths = {}
    for account, text in ((default, "default"), (work, "work")):
        path = account.home / "sessions" / "2026" / "09" / "09"
        path.mkdir(parents=True)
        rollout = path / f"rollout-2026-09-09T10-00-00-{sid}.jsonl"
        rollout.write_text(json.dumps({
            "type": "session_meta", "payload": {"session_id": sid, "cwd": "/repo"},
        }) + "\n" + text + "\n", encoding="utf-8")
        paths[account.id] = rollout
    return default, work, paths, sid


def test_codex_lista_origem_de_cada_conta(duas_contas_codex):
    default, work, paths, _ = duas_contas_codex
    convs = ap._codex_conversas()
    assert {c.codex_home for c in convs} == {
        str(default.home.resolve()), str(work.home.resolve()),
    }


def test_codex_id_duplicado_exige_conta(duas_contas_codex):
    _default, work, paths, sid = duas_contas_codex
    with pytest.raises(codex_contas.AccountError) as error:
        ap.jsonl_de("codex", sid)
    assert error.value.status == 409
    assert error.value.code == "codex_account_ambiguous_rollout"
    assert ap.jsonl_de("codex", sid, codex_account=work.id) == paths[work.id]


def test_historico_arquivado_fora_do_claude_le_linha_a_linha(tmp_path, monkeypatch):
    from app import api
    f = tmp_path / "rollout.jsonl"
    f.write_text('{"n": 1}\nlixo\n\n[1]\n{"n": 2}\n', encoding="utf-8")
    monkeypatch.setattr(api, "archive_jsonl", lambda *a, **k: f)
    monkeypatch.setattr(api.archive_providers, "parse_obj", lambda provider, o: [o["n"]])
    assert api.archive_history("p", "s", provider="codex") == [1, 2]

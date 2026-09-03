"""Conversa morta de Pi, Kimi e Codex: cada um guarda de um jeito e nenhum guarda como o Claude.

Os fixtures montam o layout REAL de cada um (medido no disco), porque e justamente o layout que o
codigo tem que acertar -- um fake "como a doc sugere" passaria sem provar nada.
"""
import json

import pytest

from app import archive_providers as ap


PI_SID = "602c251a-5233-42be-9b55-585c88e072f2"
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
    monkeypatch.setattr(pi_sessions, "sessions_root", lambda provider="pi": raiz)
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
    home = tmp_path / "codex"
    d = home / "sessions" / "2026" / "07" / "25"
    d.mkdir(parents=True)
    j = d / f"rollout-2026-07-25T12-22-09-{CODEX_SID}.jsonl"
    j.write_text(json.dumps({
        "timestamp": "2026-07-25T15:23:08.027Z", "type": "session_meta",
        "payload": {"session_id": CODEX_SID, "cwd": "/home/u/proj"},
    }) + "\n", encoding="utf-8")
    monkeypatch.setenv("CODEX_HOME", str(home))
    return j


def test_pi_lista_conversa_e_ignora_subagente(pi_home):
    convs = ap._pi_conversas()
    assert [(c.provider, c.session_id, c.cwd) for c in convs] == [("pi", PI_SID, "/home/u/proj")]
    # O caminho sai do session_id por BUSCA: o timestamp do nome nao da pra recriar.
    assert ap.jsonl_de("pi", PI_SID) == pi_home


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

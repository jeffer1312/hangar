import time
import pytest
from types import SimpleNamespace
from fastapi import FastAPI, Depends
from fastapi.testclient import TestClient
from app.auth import require_auth
from app.config import settings
from app import pair, tmux
from app import registry as registry_mod
import app.api as api_mod


@pytest.fixture(autouse=True)
def _pair_dir_isolado(tmp_path, monkeypatch):
    # registry.list() varre pareamento (Task 8) a cada chamada, inclusive dentro de rotas exercitadas
    # via TestClient — sem isto a suite varria o .hangar-pair REAL de quem roda (achado do review).
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    # api.py registra este gancho no import (dispara thread real de _drain_session, que chama
    # registry.list() de novo) — sem neutraliza-lo aqui, um sweep achando ausente de verdade nesta
    # suite dispararia essa thread fora da janela isolada (achado do review de Task 8).


@pytest.fixture
def client():
    settings.auth_token = "secret"
    app = FastAPI()

    @app.get("/ping", dependencies=[Depends(require_auth)])
    def ping():
        return {"ok": True}

    with TestClient(app) as c:
        yield c


def test_rejects_without_token(client):
    assert client.get("/ping").status_code == 401


def test_accepts_bearer(client):
    r = client.get("/ping", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200


def test_accepts_cookie(client):
    client.cookies.set("cp_token", "secret")
    r = client.get("/ping")
    assert r.status_code == 200


def test_rejects_wrong_bearer(client):
    r = client.get("/ping", headers={"Authorization": "Bearer wrong"})
    assert r.status_code == 401


def test_rejects_wrong_cookie(client):
    client.cookies.set("cp_token", "wrong")
    r = client.get("/ping")
    assert r.status_code == 401


# ---------------------------------------------------------------------------
# Route tests
# ---------------------------------------------------------------------------
from unittest.mock import ANY, patch
from app.models import SessionInfo


@pytest.fixture
def api_client(monkeypatch):
    settings.auth_token = "secret"
    # Guarda de existência do /input//broadcast (sessão morta -> 404): os testes de rota fabricam
    # sessões que não existem no tmux/Codex reais — a guarda não é o que eles testam.
    import app.api as api_mod
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    from app.api import app
    return TestClient(app)


def _h():
    return {"Authorization": "Bearer secret"}



# ---------------------------------------------------------------------------
# _send_one / Pi: entrada da fila ANTES do envio, com id estavel (achado ALTA da revisao 02/08/2026)
# ---------------------------------------------------------------------------



# ---------------------------------------------------------------------------
# Input/broadcast/interrupt por provider (Task 7 — tornar o Codex conversavel)
# ---------------------------------------------------------------------------
from unittest.mock import AsyncMock, MagicMock


def _fake_codex_adapter(deliverable=True, send_result="sent"):
    fake = MagicMock()
    fake.deliverable = AsyncMock(return_value=deliverable)
    fake.send_prompt = AsyncMock(return_value=send_result)
    fake.interrupt = AsyncMock(return_value=True)
    return fake



# ---------------------------------------------------------------------------
# Testes de config dirs (Task 4)
# ---------------------------------------------------------------------------



# ---------------------------------------------------------------------------
# GET /history?limit=N devolve a CAUDA CRUA do transcript. O corte no 1o user_msg (nao desenhar
# resposta orfa) e do card do quadro e vive no BoardCard.svelte: aqui ele valia pra TODO consumidor
# e matava a espiada do hover da Sidebar, que so quer o ultimo assistant_msg da cauda.
# ---------------------------------------------------------------------------
from app.models import ChatEvent


@pytest.fixture
def fake_session_with_transcript(api_client):
    # Transcript de 7 eventos: a cauda (limit=3) cai no meio de um turno — id5 e um assistant_msg cujo
    # prompt ficou de fora da janela. A rota devolve os 3 crus; quem corta e quem desenha bolha.
    evs = [
        ChatEvent(kind="user_msg", id="1", text="oi"),
        ChatEvent(kind="assistant_msg", id="2", text="ola"),
        ChatEvent(kind="tool_use", id="3", tool_name="Bash"),
        ChatEvent(kind="tool_result", id="4", result="ok"),
        ChatEvent(kind="assistant_msg", id="5", text="pensando"),
        ChatEvent(kind="user_msg", id="6", text="valeu"),
        ChatEvent(kind="assistant_msg", id="7", text="de nada"),
    ]
    info = SessionInfo(name="cc", cwd="/p", jsonl="/r/cc.jsonl", provider="claude")
    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.pqueue.merged_history", return_value=evs):
        yield "cc"



# ---------------------------------------------------------------------------
# GET /history condicional: entrar numa sessao e a leitura mais repetida do app e quase sempre nada
# mudou. O validador (ETag) e o que faz o celular pagar ~200 bytes em vez dos 313 KB da cauda — e o
# que dispensa qualquer regra de "quando invalidar o cache" no cliente.
# ---------------------------------------------------------------------------


@pytest.fixture
def sessao_com_jsonl_real(api_client, tmp_path):
    """Como a fixture acima, mas com um jsonl QUE EXISTE: o validador sai de `stat`, entao sem
    arquivo de verdade nao ha etag nenhum pra testar."""
    jsonl = tmp_path / "cc.jsonl"
    jsonl.write_text('{"a": 1}\n', encoding="utf-8")
    evs = [ChatEvent(kind="user_msg", id="1", text="oi"),
           ChatEvent(kind="assistant_msg", id="2", text="ola")]
    info = SessionInfo(name="cc", cwd="/p", jsonl=str(jsonl), provider="claude")
    with patch("app.api.registry.list", return_value=[info]), \
         patch("app.pqueue.merged_history", return_value=evs):
        yield "cc", jsonl



# ---------------------------------------------------------------------------
# Feature #5: corpo rico do push de awaiting (askq -> classify -> fallback) + endpoints de mute/quiet-hours
# ---------------------------------------------------------------------------
from types import SimpleNamespace


# --- POST /api/archive/{project}/{session_id}/resume: "Retomar conversa" do Arquivo ---


# --- Retomar conversa CODEX do Arquivo (ticket 08) ---
# A rota recusava com 409. O id da conversa e o uuid do fim do nome do rollout, e e ele que o
# `codex resume` recebe — o mesmo id que o Arquivo ja usa pra listar e abrir a conversa.

# ---------------------------------------------------------------------------
# Loop runner (harness bloco A — task 6)
# ---------------------------------------------------------------------------
from app import loop as loop_mod


# --- Loop goal refiner (claude -p efemero) -----------------------------------
import subprocess as _subprocess


def _completed(returncode=0, stdout="", stderr=""):
    return _subprocess.CompletedProcess(args=[], returncode=returncode, stdout=stdout, stderr=stderr)


@pytest.fixture(autouse=True)
def _claude_resolvido(monkeypatch):
    # O refinador resolve o `claude` pelo PATH; o subprocess e mockado, entao o teste nao pode
    # depender de haver um instalado (o CI nao tem).
    monkeypatch.setattr("app.loop._exe_claude", lambda: "claude")


# --- Nucleo sagrado: caminho de envio isolado do executor default ------------

# --- Nucleo: broadcast e group-message tambem no pool dedicado de envio -------

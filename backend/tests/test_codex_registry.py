"""Lifecycle da sessao Codex no registry (app-server compartilhado + TUI tmux + resume lazy).

Mocka o AppServerClient (NAO spawna o codex real). Cobre sidecar, attach, dedup da TUI na listagem,
kill dos dois processos e ensure_running pos-restart (dict vazio -> thread/resume + nova TUI)."""
import asyncio
import json
import os
from dataclasses import replace

import pytest
from unittest.mock import patch
from pathlib import Path

from app import pair, registry
from app import procinfo
from app import codex_contas as codex_accounts
from app.registry import SessionRegistry
from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex import adapter as codex_adapter
from app.adapters.codex.adapter import CodexAdapter
import codex_contas_apoio


@pytest.fixture(autouse=True)
def _isolate(tmp_path, monkeypatch):
    # Cache de classe compartilhado -> zera entre testes. Sidecars redirecionados pra tmp.
    # pair.settings.projects_dir tambem: reg.list() varre pareamento (Task 8) a cada chamada, e
    # sem isto varria o .hangar-pair REAL de quem roda (achado do review).
    from app import conversation_transfer
    monkeypatch.setattr(conversation_transfer, "_base", lambda: tmp_path / "transfers")
    SessionRegistry._jsonl_cache.clear()
    SessionRegistry._fd_locked.clear()
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(SessionRegistry, "_pair_ausencias", {})
    sdir = tmp_path / "codex-sessions"
    with patch.object(codex_sessions, "_dir", lambda: sdir), \
         patch.object(registry.tmux, "new_session", return_value=True), \
         patch.object(registry.tmux, "kill_session"):
        yield
    SessionRegistry._jsonl_cache.clear()
    SessionRegistry._fd_locked.clear()


class _FakeClient:
    """Duck-type de AppServerClient: grava requests, responde thread/start e thread/resume."""

    def __init__(self, *_args, **_kwargs):
        self.requests: list[tuple[str, dict]] = []
        self.started = False
        self.closed = False
        self.conectado = None
        self._thread_id = "019f5c00-5d7d-7dd2-b2cb-085ca6d76251"
        self._path = "/home/u/.codex/sessions/2026/07/13/rollout-x.jsonl"

    async def start(self):
        self.started = True

    async def start_shared(self, **kwargs):
        self.started = True
        return "ws://127.0.0.1:45123"

    async def connect(self, endpoint, timeout=5.0):
        self.conectado = endpoint
        return endpoint

    async def request(self, method, params, timeout=30.0):
        self.requests.append((method, params))
        if method in ("thread/start", "thread/resume"):
            return {"thread": {"id": self._thread_id, "path": self._path}, "model": "gpt-5.6-sol"}
        return {}

    async def notifications(self):
        yield {
            "method": "thread/started",
            "params": {"thread": {
                "id": self._thread_id, "path": self._path, "cwd": "/tmp/proj",
            }},
        }

    def terminate(self):
        self.closed = True

    async def close(self):
        self.closed = True


# --- Teste 1: criar sessao Codex e o caminho NORMAL, com o lancador no pane ------------------

def test_create_codex_usa_o_lancador_e_nao_pre_semeia_transcript(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.shutil, "which", return_value="/usr/bin/hangar-codex-tui"), \
         patch.object(codex_sessions, "pretrust_cwd") as pretrust, \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        info = reg.create("mysess", "/tmp/proj", provider="codex",
                          initial_prompt="revise este projeto")
    assert info.provider == "codex"
    # O rollout so nasce quando a TUI abre a thread: um path do layout do Claude aqui envenenaria o
    # _jsonl_cache, que e de classe e compartilhado com o SSE.
    assert info.jsonl is None
    assert info.tracked is False
    assert new_sess.call_args.kwargs["provider"] == "codex"
    assert "mysess" not in SessionRegistry._jsonl_cache
    comando = new_sess.call_args[0][2]
    assert "hangar-codex-tui" in comando
    assert "/tmp/proj" in comando
    assert "revise este projeto" in comando
    pretrust.assert_called_once()


@pytest.mark.parametrize("tier", [None, "priority", "default"])
def test_create_codex_tier_reaches_launcher_without_prelaunch_sidecar(tmp_path, tier):
    import shlex
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.shutil, "which", return_value="/usr/bin/hangar-codex-tui"), \
         patch.object(codex_sessions, "pretrust_cwd"), \
         patch.object(registry.tmux, "new_session", return_value=True) as spawn:
        reg.create("cx", "/tmp/proj", provider="codex", service_tier=tier, initial_prompt="primeiro prompt")
    argv = shlex.split(spawn.call_args.args[2])
    if tier is None:
        assert "--service-tier" not in argv
    else:
        assert argv[argv.index("--service-tier") + 1] == tier
        assert argv.index("--service-tier") < argv.index("--prompt")
    assert codex_sessions.load("cx") is None


@pytest.mark.parametrize("tier", [None, "priority", "default"])
def test_create_codex_headless_saves_tier_before_process_start(tmp_path, tier):
    from app.adapters.codex import sem_terminal
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(codex_sessions, "pretrust_cwd"), \
         patch.object(registry.tmux, "new_session") as spawn:
        info = reg.create("cx", "/tmp/proj", provider="codex", headless=True, service_tier=tier)
    meta = codex_sessions.load("cx")
    assert meta.get("service_tier") == tier
    assert meta["thread_id"] is None and meta["headless"] is True
    assert info.headless is True
    argv = sem_terminal.argv(meta)
    assert (f'service_tier="{tier}"' in argv) if tier else not any("service_tier=" in arg for arg in argv)
    spawn.assert_not_called()


@pytest.mark.parametrize("provider,tier", [("codex", "fast"), ("claude", "default"), ("pi", "priority")])
def test_registry_rejects_invalid_tier_before_effects(tmp_path, provider, tier):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "new_session") as spawn, \
         patch.object(registry, "_pretrust_cwd") as trust, \
         patch.object(codex_sessions, "save") as save:
        with pytest.raises(ValueError, match="service_tier"):
            reg.create("cx", "/tmp/proj", provider=provider, service_tier=tier)
    spawn.assert_not_called()
    trust.assert_not_called()
    save.assert_not_called()


def test_create_codex_transporta_a_conta_secundaria_ao_lancador(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(codex_accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    account = codex_contas_apoio.create_account("work")
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.shutil, "which", return_value="/usr/bin/hangar-codex-tui"), \
         patch.object(codex_sessions, "pretrust_cwd") as pretrust, \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        reg.create("mysess", "/tmp/proj", provider="codex", codex_account="work")
    comando = new_sess.call_args[0][2]
    assert "--codex-home" in comando
    assert str(account.home) in comando
    pretrust.assert_not_called()


def test_create_codex_com_resume_abre_a_conversa_existente(tmp_path):
    """Retomar do Arquivo: o id vai pro lancador, que abre a TUI com `codex resume <id>`. O
    historico ja esta no rollout, entao a sessao nova nasce com a conversa inteira."""
    reg = SessionRegistry(projects_dir=tmp_path)
    sid = "01a052d1-3e59-7441-9ed3-6bbd9e2704fc"
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.shutil, "which", return_value="/usr/bin/hangar-codex-tui"), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        info = reg.create("retomada", "/tmp/proj", provider="codex", resume_session_id=sid)
    assert info.provider == "codex"
    comando = new_sess.call_args[0][2]
    assert "--resume" in comando and sid in comando


def test_create_codex_recusa_id_de_conversa_invalido(tmp_path):
    """O id vai pro comando do pane: aceitar lixo aqui e mandar lixo pro shell."""
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.shutil, "which", return_value="/usr/bin/hangar-codex-tui"), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        with pytest.raises(ValueError, match="session_id invalido"):
            reg.create("retomada", "/tmp/proj", provider="codex",
                       resume_session_id="; rm -rf /")
    new_sess.assert_not_called()


def test_create_codex_recusa_quando_o_lancador_nao_esta_no_path(tmp_path):
    """Sem o lancador o pane morre no ato e o tmux devolve 0 — a sessao evaporaria calada."""
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.shutil, "which", return_value=None), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        with pytest.raises(ValueError, match="hangar-codex-tui"):
            reg.create("mysess", "/tmp/proj", provider="codex")
    new_sess.assert_not_called()


def test_create_codex_recusa_quando_o_binario_codex_nao_esta_no_path(tmp_path):
    """Mesmo desfecho do lancador ausente, por outra porta: o lancador esta la e o `codex` nao.

    Ele so chama `codex app-server` DEPOIS que o pane nasceu, entao a falta virava
    FileNotFoundError dentro do pane — que morre com o `new-session` ja tendo devolvido 0. Sem
    sidecar escrito, a sessao nao aparece nem no tmux nem em `list_all()`: sucesso reportado,
    sessao inexistente."""
    reg = SessionRegistry(projects_dir=tmp_path)
    presentes = {"hangar-codex-tui": "/usr/bin/hangar-codex-tui"}
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.shutil, "which", side_effect=presentes.get), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        with pytest.raises(ValueError, match="codex nao esta no PATH"):
            reg.create("mysess", "/tmp/proj", provider="codex")
    new_sess.assert_not_called()


def test_kill_codex_falho_nao_derruba_o_app_server_antes(tmp_path):
    """`kill_session` falhando, a sessao SEGUE viva — entao o app-server dela nao pode ter morrido.

    Na ordem antiga o `close_sync` vinha primeiro: o KillFailed dizia "nao consegui encerrar" com a
    TUI ja sem servidor, e o que sobrava na tela nao falava mais com ninguem."""
    codex_sessions.save("cx", "tid-1", "/tmp/rollout.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    adapter = CodexAdapter()
    with patch.object(registry.tmux, "kill_session", return_value=False), \
         patch("app.adapters.get_adapter", return_value=adapter), \
         patch.object(adapter, "close_sync") as close:
        with pytest.raises(registry.KillFailed):
            reg.kill("cx")
    close.assert_not_called()
    assert codex_sessions.exists("cx"), "sidecar de sessao viva nao pode ser apagado"


# --- Teste 2: list() inclui Codex (sidecar) E Claude (tmux) ---------------------------------

def test_list_includes_codex_sidecar_and_tmux(tmp_path):
    codex_sessions.save("cx", "tid-1", "/home/u/.codex/sessions/rollout-a.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    tmux_panes = {"claudesess": [{"name": "claudesess", "cwd": "/tmp/c", "pid": 111,
                                  "pane_id": "%1", "active": True}]}
    with patch.object(registry.tmux, "list_panes_all", return_value=tmux_panes), \
         patch.object(procinfo, "_proc_children_map", return_value={}), \
         patch.object(SessionRegistry, "resolve_tracked", return_value=("/x/claude.jsonl", True)), \
         patch.object(SessionRegistry, "_repl_sid", return_value=None):
        out = reg.list()
    by_name = {s.name: s for s in out}
    assert by_name["claudesess"].provider == "claude"
    cx = by_name["cx"]
    assert cx.provider == "codex"
    assert cx.jsonl == "/home/u/.codex/sessions/rollout-a.jsonl"
    assert cx.tracked is True


def test_list_does_not_duplicate_codex_tmux_tui_as_claude(tmp_path):
    codex_sessions.save("cx", "tid-1", "/rollout-a.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    panes = {"cx": [{"name": "cx", "cwd": "/tmp/a", "pid": 111, "pane_id": "%1", "active": True}]}
    with patch.object(registry.tmux, "list_panes_all", return_value=panes), \
         patch.object(procinfo, "_proc_children_map", return_value={}), \
         patch.object(SessionRegistry, "resolve_tracked") as resolve:
        out = reg.list()
    assert [(s.name, s.provider) for s in out] == [("cx", "codex")]
    resolve.assert_not_called()


# --- Teste 3: create(provider="claude") = nao-regressao (tmux, sem sidecar) -----------------

def test_create_claude_still_uses_tmux_no_sidecar(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        info = reg.create("claudesess", "/tmp/proj")
    assert info.provider == "claude"
    new_sess.assert_called_once()  # caminho tmux intacto
    # nenhum sidecar Codex gravado
    assert codex_sessions.load("claudesess") is None


# --- Teste 4: kill de Codex fecha o client (mock) e apaga o sidecar -------------------------

def test_kill_codex_closes_client_and_removes_sidecar(tmp_path):
    codex_sessions.save("cx", "tid-1", "/home/u/.codex/rollout-a.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    fake = _FakeClient()
    adapter = CodexAdapter()
    adapter.attach("cx", fake, "tid-1")
    # Task 6 (achado da revisao, rodada 2, Quebra 2): o kill do shell escondido agora e GATEADO
    # por `tmux.is_hidden` -- so mata "term-cx" se a marca confirmar que e nosso, senao uma sessao
    # de TERCEIRO chamada "term-cx" seria derrubada junto. `is_hidden` mockado True aqui pra
    # exercitar o caminho em que o shell E nosso (o caso comum).
    with patch("app.adapters.get_adapter", return_value=adapter), \
         patch.object(registry.tmux, "kill_session") as kill_tmux, \
         patch.object(registry.tmux, "is_hidden", return_value=True):
        reg.kill("cx")
    assert fake.closed is True                      # client vivo terminado
    assert "cx" not in adapter._sessions            # esquecido da memoria
    assert codex_sessions.load("cx") is None        # sidecar duravel apagado
    kill_tmux.assert_any_call("cx")                  # encerra tambem a TUI Codex
    kill_tmux.assert_any_call("term-cx")             # e o shell escondido do painel de terminal
    assert kill_tmux.call_count == 2


def test_rename_codex_moves_sidecar_and_live_adapter(tmp_path):
    codex_sessions.save("old", "tid-1", "/rollout-a.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    adapter = CodexAdapter()
    fake = _FakeClient()
    adapter.attach("old", fake, "tid-1")
    with patch("app.adapters.get_adapter", return_value=adapter):
        reg.rename("old", "new")
    assert codex_sessions.load("old") is None
    assert codex_sessions.load("new")["name"] == "new"
    assert "old" not in adapter._sessions
    assert adapter._sessions["new"]["client"] is fake


def test_sidecar_preserva_codex_home_em_update_e_rename(tmp_path):
    codex_sessions.save("one", "thread-a", "/tmp/rollout-a.jsonl", "/tmp/project",
                        codex_home="/tmp/codex-work")
    codex_sessions.update_model("one", "model-a", "high")
    assert codex_sessions.load("one")["codex_home"] == "/tmp/codex-work"
    codex_sessions.rename("one", "two")
    assert codex_sessions.load("two")["codex_home"] == "/tmp/codex-work"


def test_list_codex_preserva_conta_do_sidecar_sem_ler_cota(tmp_path, monkeypatch):
    from app import cotas
    home = tmp_path / "codex-work"
    codex_sessions.save("cx", "thread-a", str(home / "sessions" / "rollout.jsonl"), "/tmp/a",
                        codex_home=home)
    monkeypatch.setattr(cotas, "id_conta_codex",
                        lambda *args, **kwargs: pytest.fail("list nao pode ler cota"), raising=False)
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "list_panes_all", return_value={}):
        info = next(item for item in reg.list() if item.name == "cx")
    assert info.conta == f"codex:{home.resolve()}"


def test_rename_avisa_lease_codex_antes_do_sidecar(tmp_path):
    avisos = []
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry, "apos_renomear_codex", lambda old, new: avisos.append((old, new))), \
         patch.object(registry.tmux, "is_hidden", return_value=False), \
         patch.object(registry.tmux, "rename_session", return_value=True):
        reg.rename("old", "new")
    assert avisos == [("old", "new")]


# --- Colisao de nome cross-provider (review Important #1) ------------------------------------

def test_create_claude_rejects_existing_codex_name(tmp_path):
    codex_sessions.save("dup", "tid-1", "/home/u/.codex/rollout.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        with pytest.raises(ValueError):
            reg.create("dup", "/tmp/proj")
    new_sess.assert_not_called()  # nao chegou a spawnar pane tmux orfao


async def test_ensure_running_conecta_no_app_server_do_pane(tmp_path):
    """Sidecar com endpoint+pid: o backend se LIGA ao servidor do pane, sem spawnar nem recriar TUI."""
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a",
                        endpoint="ws://127.0.0.1:45999", app_pid=4242)
    adapter = CodexAdapter()
    fake = _FakeClient()
    with patch.object(codex_adapter, "AppServerClient", lambda *a, **k: fake), \
         patch.object(codex_adapter, "pid_vivo", return_value=True), \
         patch.object(codex_adapter, "ensure_tmux_tui") as tui:
        client = await adapter.ensure_running("cx")
    assert client is fake
    assert fake.conectado == "ws://127.0.0.1:45999"
    assert fake.started is False     # nao subiu app-server nenhum
    tui.assert_not_called()          # a TUI ja esta viva no pane; recriar mataria a conversa na tela
    assert "cx" in adapter._sessions


async def test_ensure_running_com_app_server_morto_e_sessao_morta(tmp_path):
    """Pid morto: nao adianta tentar o endereco — porta de loopback e reciclada e o outro lado
    pode ser um processo alheio. A sessao acabou, e dizer isso e melhor que reconectar as cegas."""
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a",
                        endpoint="ws://127.0.0.1:45999", app_pid=4242)
    adapter = CodexAdapter()
    fake = _FakeClient()
    with patch.object(codex_adapter, "AppServerClient", lambda *a, **k: fake), \
         patch.object(codex_adapter, "pid_vivo", return_value=False):
        assert await adapter.ensure_running("cx") is None
    assert fake.conectado is None
    assert "cx" not in adapter._sessions


async def test_pane_vivo_sem_controle_nunca_e_substituido(tmp_path):
    """Reiniciar o backend nao pode custar o pane de quem esta trabalhando na TUI.

    Sidecar do desenho antigo (sem endpoint) + pane vivo: o caminho de recriar a TUI mataria o pane
    para pendurar outro no lugar. Aqui a resposta e "sessao sem controle vivo" — quem abriu o chat
    ve o estado morto, que tem tela propria, e o pane fica intocado."""
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a")   # sem endpoint/app_pid
    adapter = CodexAdapter()
    with patch.object(codex_adapter.tmux, "has_session", return_value=True), \
         patch.object(codex_adapter, "AppServerClient") as cliente, \
         patch.object(codex_adapter, "ensure_tmux_tui") as tui:
        assert await adapter.ensure_running("cx") is None
    cliente.assert_not_called()   # nem chega a spawnar um app-server
    tui.assert_not_called()       # e muito menos a derrubar o pane


async def test_sem_pane_o_resume_do_desenho_antigo_ainda_recria(tmp_path):
    """A guarda acima nao pode matar o resume: sem pane nao ha tela de ninguem pra destruir, e
    reabrir a conversa pelo app continua valendo."""
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a")
    adapter = CodexAdapter()
    fake = _FakeClient()
    with patch.object(codex_adapter.tmux, "has_session", return_value=False), \
         patch.object(codex_adapter, "AppServerClient", lambda *a, **k: fake), \
         patch.object(codex_adapter, "ensure_tmux_tui") as tui:
        assert await adapter.ensure_running("cx") is fake
    assert tui.call_args.kwargs["replace"] is True
    assert [m for m, _ in fake.requests] == ["initialize", "thread/resume"]


async def test_resume_legacy_usa_codex_home_do_sidecar(tmp_path):
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a",
                        codex_home="/tmp/codex-work")
    adapter = CodexAdapter()
    fake = _FakeClient()
    with patch.object(codex_adapter.tmux, "has_session", return_value=False), \
         patch.object(codex_adapter, "AppServerClient", lambda *a, **k: fake), \
         patch.object(codex_adapter, "ensure_tmux_tui") as tui:
        assert await adapter.ensure_running("cx") is fake
    assert tui.call_args.kwargs["codex_home"] == "/tmp/codex-work"


async def test_app_server_morto_com_pane_vivo_nao_tira_a_sessao_da_lista(tmp_path):
    """A cadeia inteira do segundo critério, num teste só.

    App-server morto e pane vivo: `ensure_running` desiste (quem abriu o chat vê o estado morto,
    que tem tela própria), o pane NÃO é tocado, e a sessão continua listada como ociosa. O board tem
    três colunas fixas e nenhuma de morto — sumir da lista faria o card desaparecer enquanto a
    pessoa trabalha na TUI, que é pior que o defeito que este ticket corrige."""
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a",
                        endpoint="ws://127.0.0.1:1", app_pid=4242)
    adapter = CodexAdapter()
    with patch.object(codex_adapter, "pid_vivo", return_value=False), \
         patch.object(codex_adapter.tmux, "has_session", return_value=True), \
         patch.object(codex_adapter, "AppServerClient") as cliente, \
         patch.object(codex_adapter, "ensure_tmux_tui") as tui:
        assert await adapter.ensure_running("cx") is None
    cliente.assert_not_called()
    tui.assert_not_called()

    # E o sidecar segue no disco, entao a sessao segue na lista. `_watch_tmux` e o unico que apaga,
    # e ele so age quando o PANE some — nunca por causa do app-server.
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "list_panes_all", return_value={}), \
         patch.object(procinfo, "_proc_children_map", return_value={}):
        out = reg.list()
    assert [(s.name, s.provider) for s in out] == [("cx", "codex")]


def test_pane_codex_sem_sidecar_nao_vira_sessao_claude(tmp_path):
    """A janela entre o pane nascer e o lancador gravar o sidecar tem dono.

    Sem isto o pane cai no default "claude" e e casado com o transcript do Claude do mesmo
    diretorio — a regressao que ja custou caro no Pi."""
    reg = SessionRegistry(projects_dir=tmp_path)
    panes = {"cx": [{"name": "cx", "cwd": "/tmp/a", "pid": 321, "pane_id": "%3", "active": True}]}
    with patch.object(registry.tmux, "list_panes_all", return_value=panes), \
         patch.object(procinfo, "_proc_children_map", return_value={}), \
         patch.object(registry, "agente_do_pane", return_value=("codex", 111)), \
         patch.object(SessionRegistry, "resolve_tracked") as resolve, \
         patch.object(SessionRegistry, "_repl_sid", return_value=None):
        out = reg.list()
    resolve.assert_not_called()   # nem chega a procurar transcript do Claude
    assert [(s.name, s.provider, s.jsonl, s.tracked) for s in out] == [("cx", "codex", None, False)]


@pytest.mark.parametrize("detected", [("claude", None), ("kimi", 111)])
def test_provider_de_nascimento_cobre_o_shell_sem_impedir_troca(tmp_path, monkeypatch, detected):
    reg = SessionRegistry(projects_dir=tmp_path)
    reg._jsonl_cache["cx"] = str(tmp_path / "claude-antigo.jsonl")
    panes = {"cx": [{"name": "cx", "cwd": str(tmp_path), "pid": 321,
                     "pane_id": "%3", "active": True, "provider": "codex"}]}
    monkeypatch.setattr(registry.tmux, "list_panes_all", lambda: panes)
    monkeypatch.setattr(registry, "_proc_children_map", lambda: {})
    monkeypatch.setattr(registry, "agente_do_pane", lambda *args: detected)
    monkeypatch.setattr(registry, "kimi_session_file", lambda *args: None)
    with patch.object(SessionRegistry, "resolve_tracked", return_value=(reg._jsonl_cache["cx"], True)) as resolve:
        out = reg.list()
    resolve.assert_not_called()
    assert [(s.provider, s.jsonl, s.tracked) for s in out] == [
        ("codex" if detected[1] is None else "kimi", None, False)]


@pytest.mark.parametrize("argv", [
    ["/usr/bin/python3", "/home/u/.local/bin/hangar-codex-tui", "--cwd", "/tmp/a"],
    ["/repo/backend/.venv/bin/python", "/repo/scripts/hangar-codex-tui", "--cwd", "/tmp/a"],
    ["/repo/scripts/hangar-codex-tui", "--cwd", "/tmp/a"],
])
def test_lancador_em_integracao_nao_herda_transcript_por_nome(tmp_path, monkeypatch, argv):
    reg = SessionRegistry(projects_dir=tmp_path)
    reg._jsonl_cache["cx"] = str(tmp_path / "claude-antigo.jsonl")
    reg._fd_locked.add("cx")
    panes = {"cx": [{"name": "cx", "cwd": "/tmp/a", "pid": 321, "pane_id": "%3", "active": True}]}
    monkeypatch.setattr(registry.tmux, "list_panes_all", lambda: panes)
    monkeypatch.setattr(registry, "_proc_children_map", lambda: {})
    monkeypatch.setattr(registry, "_descendant_pids", lambda *args: [321])
    monkeypatch.setattr(registry, "_argv", lambda pid: argv)
    monkeypatch.setattr(registry, "_cmdline", lambda pid: " ".join(argv))
    with patch.object(SessionRegistry, "resolve_tracked") as resolve:
        out = reg.list()
    resolve.assert_not_called()
    assert [(s.provider, s.jsonl, s.tracked) for s in out] == [("codex", None, False)]


@pytest.mark.parametrize("argv", [
    ["python3", "outro.py", "hangar-codex-tui"],
    ["python3", "-c", "hangar-codex-tui"],
    ["cat", "/repo/scripts/hangar-codex-tui"],
])
def test_argumento_citando_lancador_nao_e_agente(argv):
    assert registry._provider_do_argv(argv) is None


async def test_preparacao_codex_publica_etapa_atual_sem_cache_anterior(tmp_path, monkeypatch):
    from pathlib import Path
    from app.sse import _list_sig

    reg = SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(reg, "_status_cache", {"cx": (registry.time.monotonic(), "modelo antigo")})
    reg._jsonl_cache["cx"] = "/claude-antigo.jsonl"
    quadros = iter([
        "hangar-codex-tui: integração Codex: Verificando plugins\n",
        "hangar-codex-tui: integração Codex: Verificando plugins\n"
        "hangar-codex-tui: integração Codex: Instalando plugin\n",
        (Path(__file__).parent / "fixtures" / "pane_codex_hooks.txt").read_text(),
    ])
    monkeypatch.setattr(registry.tmux, "capture_pane", lambda *args: next(quadros))
    assinaturas = []
    for estado, etapa in [("working", "Verificando plugins"), ("working", "Instalando plugin"),
                          ("awaiting_input", None)]:
        info = registry.SessionInfo(name="cx", cwd=None, jsonl=None, tracked=False, provider="codex")
        out = await reg.list_with_state([info])
        assert out[0].state == estado
        assert out[0].label == (f"integração Codex: {etapa}" if etapa else None)
        expected = (["integração Codex: Verificando plugins"] if etapa else [
            "Hooks alterados: confira a aprovação dos hooks no Codex antes de usá-los.",
            "há itens aguardando sua confirmação de confiança no Codex",
        ])
        if etapa == "Instalando plugin":
            expected.append("integração Codex: Instalando plugin")
        assert out[0].startup_steps == expected
        assert out[0].status_line is None
        assert "cx" not in reg._jsonl_cache
        assinaturas.append(_list_sig(out))
    assert assinaturas[0] != assinaturas[1]
    same_label = registry.SessionInfo(name="cx", provider="codex", tracked=False,
                                      startup_steps=["etapa anterior"])
    assert _list_sig([same_label]) != _list_sig([same_label.model_copy(update={"startup_steps": []})])
    assert out[0].options == ["Review hooks", "Trust all and continue",
                              "Continue without trusting (hooks won't run)"]


@pytest.mark.parametrize("failed", [True, False])
async def test_codex_abertura_fatal_nao_fica_trabalhando(tmp_path, monkeypatch, failed):
    reg = SessionRegistry(projects_dir=tmp_path)
    message = "sincronização da conta falhou; Codex não foi aberto" if failed else "integração falhou; a sessão abre assim mesmo"
    frame = f"hangar-codex-tui: {message}\n" + ("Pressione Enter para fechar esta sessão.\n" if failed else "")
    monkeypatch.setattr(registry.tmux, "capture_pane", lambda *args: frame)
    info = registry.SessionInfo(name="cx", provider="codex", jsonl=None, tracked=False)
    out = await reg.list_with_state([info])
    assert out[0].state == ("awaiting_input" if failed else "working")
    assert out[0].problema == ("codex_abertura_falhou" if failed else None)
    assert out[0].label == message


def test_git_codex_atualiza_lista_mesmo_sem_novo_turno():
    from app.sse import _list_sig

    info = registry.SessionInfo(name="cx", provider="codex", branch="main", git_dirty=0)
    original = _list_sig([info])
    assert _list_sig([info.model_copy(update={"branch": "fix"})]) != original
    assert _list_sig([info.model_copy(update={"git_dirty": 1})]) != original


def _config_codex(tmp_path, conteudo):
    casa = tmp_path / "casa"
    (casa / ".codex").mkdir(parents=True)
    cfg = casa / ".codex" / "config.toml"
    cfg.write_text(conteudo, encoding="utf-8")
    return casa, cfg


def test_pretrust_escreve_antes_do_bloco_gerenciado(tmp_path):
    """A entrada nao pode cair no fim do arquivo: o fim, hoje, esta DENTRO do bloco que a ponte de
    skills reescreve inteiro (o Codex apenda a confianca dos hooks la)."""
    casa, cfg = _config_codex(tmp_path, 'model = "x"\n\n# >>> hangar: provedor\nfoo = 1\n')
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/pasta-nova")
    texto = cfg.read_text()
    assert texto.index('[projects."/tmp/pasta-nova"]') < texto.index("# >>> hangar:")
    assert texto.endswith("foo = 1\n")     # o bloco de terceiro segue intacto no fim


@pytest.mark.skipif(os.name != "posix", reason="modo de arquivo POSIX")
def test_pretrust_preserva_o_modo_do_config(tmp_path):
    """O config do Codex guarda credencial de provedor. Um pretrust nao pode afrouxar o arquivo
    pro umask so por ter passado por um temporario."""
    casa, cfg = _config_codex(tmp_path, 'model = "x"\n')
    cfg.chmod(0o600)
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/pasta-nova")
    assert cfg.stat().st_mode & 0o777 == 0o600


def test_pretrust_e_idempotente(tmp_path):
    casa, cfg = _config_codex(tmp_path, '[projects."/tmp/ja"]\ntrust_level = "trusted"\n')
    antes = cfg.read_text()
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/ja")
    assert cfg.read_text() == antes


def test_pretrust_nao_redefine_tabela_escrita_de_outra_forma(tmp_path):
    """Uma checagem por regex de `[projects."<alvo>"]` nao veria esta forma, e apendar a nossa
    REDEFINIRIA a tabela: o config pararia de abrir, pro Codex e pra ponte."""
    casa, cfg = _config_codex(tmp_path, "[projects]\n'/tmp/ja' = { trust_level = 'trusted' }\n")
    antes = cfg.read_text()
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/ja")
    assert cfg.read_text() == antes


def test_pretrust_nao_derruba_a_criacao_com_config_quebrado(tmp_path):
    casa, cfg = _config_codex(tmp_path, "isto ] nao [ e toml\n")
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/pasta-nova")   # best-effort: nao levanta
    assert cfg.read_text() == "isto ] nao [ e toml\n"    # e nao corrompe mais ainda


def test_kill_manda_sigterm_no_pid_do_app_server(tmp_path):
    """Encerrar pelo app tem que matar o app-server POR PID.

    Ele nao e mais filho do backend, entao o `client.terminate()` do desenho antigo nao tem
    processo pra matar — parar nele deixaria o servidor escutando em loopback sem dono, que e
    exatamente o orfao que ja aconteceu nesta maquina."""
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a",
                        endpoint="ws://127.0.0.1:45999", app_pid=4242)
    mortos = []
    argv = ["codex", "app-server", "--listen", "ws://127.0.0.1:45999"]
    with patch.object(codex_adapter, "pid_vivo", return_value=True), \
         patch.object(codex_adapter, "_argv", return_value=argv), \
         patch.object(codex_adapter.os, "kill", lambda pid, sig: mortos.append((pid, sig))):
        codex_adapter.matar_app_server("cx")
    assert mortos == [(4242, codex_adapter.signal.SIGTERM)]


@pytest.mark.parametrize("argv", [["/usr/bin/firefox"],
                                  ["codex", "app-server", "--listen", "ws://127.0.0.1:1111"]])
def test_kill_nao_mata_pid_reaproveitado_vivo(tmp_path, argv):
    """Depois de reiniciar a máquina o pid do sidecar pode estar vivo em outro processo."""
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a",
                        endpoint="ws://127.0.0.1:45999", app_pid=4242)
    mortos = []
    with patch.object(codex_adapter, "pid_vivo", return_value=True), \
         patch.object(codex_adapter, "_argv", return_value=argv), \
         patch.object(codex_adapter.os, "kill", lambda pid, sig: mortos.append((pid, sig))):
        codex_adapter.matar_app_server("cx")
    assert mortos == []


def test_kill_nao_manda_sinal_pra_pid_morto(tmp_path):
    """Pid reciclado e de outra pessoa: mandar SIGTERM as cegas mataria processo alheio."""
    codex_sessions.save("cx", "tid-1", "/x/rollout.jsonl", "/tmp/a",
                        endpoint="ws://127.0.0.1:45999", app_pid=4242)
    mortos = []
    with patch.object(codex_adapter, "pid_vivo", return_value=False), \
         patch.object(codex_adapter.os, "kill", lambda pid, sig: mortos.append((pid, sig))):
        codex_adapter.matar_app_server("cx")
    assert mortos == []


def test_create_codex_rejects_existing_tmux_name(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=True), \
         patch.object(registry.shutil, "which", return_value="/usr/bin/hangar-codex-tui"), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        with pytest.raises(ValueError):
            reg.create("dup", "/tmp/proj", provider="codex")
    new_sess.assert_not_called()


# --- Teste 5: ensure_running pos-restart reabre client e retoma pelo thread_id --------------

async def test_ensure_running_resumes_by_thread_id(tmp_path):
    # Simula pos-restart: sidecar no disco, dict de clients vazio.
    codex_sessions.save("cx", "tid-42", "/home/u/.codex/rollout-a.jsonl", "/tmp/a")
    adapter = CodexAdapter()
    assert "cx" not in adapter._sessions
    fake = _FakeClient()
    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: fake):
        client = await adapter.ensure_running("cx")
    assert client is fake
    assert fake.started is True
    methods = [m for m, _ in fake.requests]
    assert "initialize" in methods
    # RETOMA o thread existente via thread/resume passando o threadId do sidecar
    assert "thread/resume" in methods
    resume_params = next(p for m, p in fake.requests if m == "thread/resume")
    assert resume_params["threadId"] == "tid-42"
    # anexado na memoria pra proximas chamadas
    assert "cx" in adapter._sessions


async def test_ensure_running_reuses_live_client(tmp_path):
    adapter = CodexAdapter()
    fake = _FakeClient()
    adapter.attach("cx", fake, "tid-1")
    client = await adapter.ensure_running("cx")
    assert client is fake
    assert fake.started is False  # nao reabriu nada


# --- IMPORTANT 1: lock por-nome evita spawn duplicado em ensure_running concorrente ----------

async def test_ensure_running_concurrent_calls_spawn_once(tmp_path):
    # 2 chamadores concorrentes pro mesmo nome, sem client vivo (pos-restart): sem lock, ambos
    # passavam pelo `sess is None`, ambos spawnavam+resumiam, e o 2o attach() sobrescrevia o 1o
    # AppServerClient no dict -- o 1o (subprocess + reader task) ficava orfao, nunca fechado.
    codex_sessions.save("cx-race", "tid-race", "/home/u/.codex/rollout-race.jsonl", "/tmp/race")
    adapter = CodexAdapter()
    starts = 0

    class _SlowFakeClient(_FakeClient):
        async def start_shared(self):
            nonlocal starts
            starts += 1
            await asyncio.sleep(0)  # forca o interleaving entre as 2 corrotinas
            self.started = True
            return "ws://127.0.0.1:45123"

    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: _SlowFakeClient()):
        c1, c2 = await asyncio.gather(
            adapter.ensure_running("cx-race"), adapter.ensure_running("cx-race"),
        )
    assert starts == 1          # client.start() chamado 1x so
    assert c1 is c2             # os dois chamadores recebem o MESMO client
    assert len(adapter._sessions) == 1


async def test_ensure_running_double_check_skips_spawn_if_attached_meanwhile(tmp_path):
    # Versao deterministica (sem depender de timing real de interleaving): segura o lock na mao,
    # dispara ensure_running numa task (que bloqueia em `async with lock`), anexa a sessao como se
    # OUTRO chamador tivesse vencido a corrida, e so entao libera o lock -- o double-check tem que
    # ver a sessao ja anexada e NAO spawnar de novo.
    codex_sessions.save("cx-dc", "tid-dc", "/home/u/.codex/rollout-dc.jsonl", "/tmp/dc")
    adapter = CodexAdapter()
    fake_first = _FakeClient()
    lock = adapter._locks.setdefault("cx-dc", asyncio.Lock())
    await lock.acquire()
    task = asyncio.create_task(adapter.ensure_running("cx-dc"))
    await asyncio.sleep(0)  # deixa a task bloquear em `async with lock`
    adapter.attach("cx-dc", fake_first, "tid-dc")
    lock.release()
    with patch("app.adapters.codex.adapter.AppServerClient",
               lambda *a, **k: pytest.fail("nao deveria spawnar de novo")):
        client = await task
    assert client is fake_first


# --- Dead-detection: state_monitor emite "dead" quando o app-server morre --------------------

async def test_state_monitor_emits_dead_on_client_close():
    from app.state import StateEvent

    class _DyingClient:
        closed = False

        async def notifications(self):
            yield {"method": "turn/started", "params": {}}
            self.closed = True  # app-server morreu (EOF) apos essa notification
            return

    adapter = CodexAdapter()
    adapter.attach("sess", _DyingClient(), "t")
    events = [ev async for ev in adapter.state_monitor("sess", lambda: "sess")]
    assert events[-1] == StateEvent(session="sess", state="dead")


@pytest.mark.parametrize("headless", [False, True])
def test_sidecar_save_keeps_transfer_budget_and_key_on_same_thread(tmp_path, headless):
    codex_sessions.save("s", "t", "/tmp/rollout", "/tmp/project", key="original", headless=headless,
                        transfer_id="transfer-1", tool_output_token_limit=144000, codex_home=tmp_path / "account")
    codex_sessions.update_model("s", "model", "high")
    codex_sessions.update_app_pid("s", 123)
    codex_sessions.save("s", "t", "/tmp/rollout", "/tmp/project", headless=headless)
    meta = codex_sessions.load("s")
    assert meta["key"] == "original"
    assert meta["transfer_id"] == "transfer-1"
    assert meta["tool_output_token_limit"] == 144000
    assert meta["codex_home"] == str(tmp_path / "account")
    codex_sessions.rename("s", "renamed")
    assert codex_sessions.load("renamed")["transfer_id"] == "transfer-1"


@pytest.mark.parametrize("tier", ["priority", "default"])
@pytest.mark.parametrize("headless", [False, True])
def test_service_tier_survives_same_thread_save_and_rename(tier, headless):
    codex_sessions.save("s", "t", "/rollout", "/project", service_tier=tier, headless=headless)
    codex_sessions.update_model("s", "model", "high")
    codex_sessions.save("s", "t", "/rollout", "/project", headless=headless)
    assert codex_sessions.load("s")["service_tier"] == tier
    codex_sessions.rename("s", "renamed")
    assert codex_sessions.load("renamed")["service_tier"] == tier
    codex_sessions.save("renamed", "new", "/new-rollout", "/project")
    assert codex_sessions.load("renamed").get("service_tier") is None


def test_service_tier_old_thread_cannot_update_new_sidecar():
    codex_sessions.save("s", "old", "/rollout", "/project", service_tier="priority")
    codex_sessions.update("s", thread_id="new")
    assert codex_sessions.load("s").get("service_tier") is None
    assert not codex_sessions.update_service_tier("s", "old", "priority")
    assert codex_sessions.load("s").get("service_tier") is None
    assert codex_sessions.update_service_tier("s", "new", "default")
    assert codex_sessions.load("s")["service_tier"] == "default"


def test_list_signature_changes_when_fast_changes():
    from app.models import SessionInfo, StateEvent
    from app.sse import _list_sig
    info = SessionInfo(name="s", provider="codex", codex_service_tier="default")
    before = _list_sig([info])
    info.codex_service_tier = "priority"
    assert _list_sig([info]) != before
    assert StateEvent(session="s", state="idle", codex_service_tier="priority").model_dump()["codex_service_tier"] == "priority"


@pytest.mark.parametrize("mutation", ["save", "update", "switch_thread"])
def test_new_thread_removes_active_import_association(tmp_path, mutation):
    codex_sessions.save("s", "t", "/tmp/rollout", "/tmp/project", key="original",
                        transfer_id="transfer-1", tool_output_token_limit=144000,
                        endpoint="ws://127.0.0.1:9", app_pid=123)
    if mutation == "save":
        codex_sessions.save("s", "new", "/tmp/new-rollout", "/tmp/project")
    elif mutation == "update":
        codex_sessions.update("s", thread_id="new", rollout_path="/tmp/new-rollout")
    else:
        assert codex_sessions.switch_thread("s", {"id": "new", "path": "/tmp/new-rollout",
                                                   "cwd": "/tmp/project", "source": "cli"},
                                           "t", endpoint="ws://127.0.0.1:9", app_pid=123)
    meta = codex_sessions.load("s")
    assert meta["key"] == "original" and meta["thread_id"] == "new"
    assert "transfer_id" not in meta and "tool_output_token_limit" not in meta


def test_incomplete_sidecar_is_not_warmed_and_stays_visible_to_registry(tmp_path):
    import uuid
    from app import conversation_transfer as transfers
    record = transfers.TransferRecord(str(uuid.uuid4()), "s", "k:old", transfers.TransferPhase.IMPORTED,
                                      None, {"name": "s", "key": "old", "cwd": str(tmp_path)},
                                      {"codex_home": str(tmp_path / "account"), "thread_id": "t"}, None, None)
    transfers.save_transfer(record)
    codex_sessions.save("s", "t", "/tmp/rollout", str(tmp_path), key="old", transfer_id=record.id,
                        codex_home=tmp_path / "account")
    assert codex_sessions.list_all() == []
    assert len(codex_sessions.list_all(include_incomplete=True)) == 1
    # Índice por conta/thread também bloqueia a preparação antes de publicar transfer_id.
    codex_sessions.update("s", transfer_id=None)
    assert codex_sessions.list_all() == []
    transfers.save_transfer(replace(record, phase=transfers.TransferPhase.COMPLETE))
    assert len(codex_sessions.list_all()) == 1


def test_transfer_record_corruption_is_not_silently_skipped(tmp_path):
    import uuid
    from app import conversation_transfer as transfers
    transfer_id = str(uuid.uuid4())
    transfers._base().mkdir()
    transfers._record_path(transfer_id).write_text("null")
    codex_sessions.save("s", "t", "/tmp/rollout", str(tmp_path), transfer_id=transfer_id)
    with pytest.raises(ValueError):
        codex_sessions.list_all()


async def test_legacy_restart_keeps_budget_account_and_resume_request(tmp_path):
    import hashlib
    import uuid
    from app import conversation_transfer as transfers
    home = str(tmp_path / "account")
    transfer_id = str(uuid.uuid4())
    thread_id = str(uuid.uuid4())
    rollout = tmp_path / "account" / "sessions" / f"rollout-test-{thread_id}.jsonl"
    rollout.parent.mkdir(parents=True)
    prefix = (json.dumps({"type": "session_meta", "payload": {"id": thread_id}}) + "\n").encode()
    rollout.write_bytes(prefix)
    source = tmp_path / "source.jsonl"
    source.write_text(json.dumps({"type": "user", "uuid": "u", "parentUuid": None,
                                  "message": {"role": "user", "content": "histórico"}}) + "\n")
    record = transfers.TransferRecord(transfer_id, "cx", "k:original-key", transfers.TransferPhase.COMPLETE,
        transfers.ConversationSource(str(source), "claude", hashlib.sha256(source.read_bytes()).hexdigest(), ("u",)),
        {"name": "cx", "key": "original-key", "cwd": str(tmp_path)},
        {"codex_home": home, "codex_account": "work", "thread_id": thread_id,
         "rollout_path": str(rollout), "tool_output_token_limit": 144000},
        transfers.ImportBoundary(thread_id, str(rollout), len(prefix), (), hashlib.sha256(prefix).hexdigest()), None)
    transfers.save_transfer(record)
    codex_sessions.save("cx", thread_id, str(rollout), str(tmp_path),
                        codex_home=home, codex_account="work", tool_output_token_limit=144000,
                        transfer_id=transfer_id, key="original-key", model="native-model", effort="low")
    codex_sessions.update("cx", permission_mode="Ask for approval")
    fake = _FakeClient()
    fake._thread_id = thread_id
    fake._path = str(rollout)
    from unittest.mock import AsyncMock
    previous_request = fake.request
    async def request(method, params, timeout=30.0):
        if method == "thread/read":
            fake.requests.append((method, params))
            return {"thread": {"id": thread_id, "model": "native-model", "reasoningEffort": "low"}}
        return await previous_request(method, params, timeout)
    fake.request = request
    fake.start_shared = AsyncMock(return_value="ws://127.0.0.1:45123")
    with patch.object(codex_adapter.tmux, "has_session", return_value=False), \
         patch.object(codex_adapter, "AppServerClient", lambda: fake), \
         patch.object(codex_adapter, "ensure_tmux_tui") as tui:
        adapter = CodexAdapter()
        await adapter.ensure_running("cx")
    fake.start_shared.assert_awaited_once_with(codex_home=home, tool_output_token_limit=144000,
                                               session_name="cx", session_key="original-key")
    assert tui.call_args.kwargs["codex_account"] == "work"
    assert next(p for m, p in fake.requests if m == "thread/resume")["threadId"] == thread_id
    assert next(p for m, p in fake.requests if m == "thread/resume")["approvalPolicy"] == "on-request"
    assert "turn/start" not in [m for m, _ in fake.requests]
    assert transfers.transfer_for_session("cx").id == transfer_id
    assert transfers.load_transfer(transfer_id).phase == transfers.TransferPhase.COMPLETE

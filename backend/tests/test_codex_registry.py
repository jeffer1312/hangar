"""Lifecycle da sessao Codex no registry (app-server compartilhado + TUI tmux + resume lazy).

Mocka o AppServerClient (NAO spawna o codex real). Cobre sidecar, attach, dedup da TUI na listagem,
kill dos dois processos e ensure_running pos-restart (dict vazio -> thread/resume + nova TUI)."""
import asyncio
import json
import os

import pytest
from unittest.mock import patch

from app import registry
from app import procinfo
from app.registry import SessionRegistry
from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex import adapter as codex_adapter
from app.adapters.codex.adapter import CodexAdapter


@pytest.fixture(autouse=True)
def _isolate(tmp_path):
    # Cache de classe compartilhado -> zera entre testes. Sidecars redirecionados pra tmp.
    SessionRegistry._jsonl_cache.clear()
    SessionRegistry._fd_locked.clear()
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

    async def start_shared(self):
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
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        info = reg.create("mysess", "/tmp/proj", provider="codex",
                          initial_prompt="revise este projeto")
    assert info.provider == "codex"
    # O rollout so nasce quando a TUI abre a thread: um path do layout do Claude aqui envenenaria o
    # _jsonl_cache, que e de classe e compartilhado com o SSE.
    assert info.jsonl is None
    assert "mysess" not in SessionRegistry._jsonl_cache
    comando = new_sess.call_args[0][2]
    assert "hangar-codex-tui" in comando
    assert "/tmp/proj" in comando
    assert "revise este projeto" in comando


def test_create_codex_recusa_quando_o_lancador_nao_esta_no_path(tmp_path):
    """Sem o lancador o pane morre no ato e o tmux devolve 0 — a sessao evaporaria calada."""
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.shutil, "which", return_value=None), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        with pytest.raises(ValueError, match="hangar-codex-tui"):
            reg.create("mysess", "/tmp/proj", provider="codex")
    new_sess.assert_not_called()


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
         patch.object(registry, "provider_of_pane", return_value="codex"), \
         patch.object(SessionRegistry, "resolve_tracked") as resolve, \
         patch.object(SessionRegistry, "_repl_sid", return_value=None):
        out = reg.list()
    resolve.assert_not_called()   # nem chega a procurar transcript do Claude
    assert [(s.name, s.provider, s.jsonl, s.tracked) for s in out] == [("cx", "codex", None, False)]


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
    with patch.object(codex_adapter, "pid_vivo", return_value=True), \
         patch.object(codex_adapter.os, "kill", lambda pid, sig: mortos.append((pid, sig))):
        codex_adapter.matar_app_server("cx")
    assert mortos == [(4242, codex_adapter.signal.SIGTERM)]


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

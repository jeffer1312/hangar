"""Contrato do importador nativo com processos locais sem acesso à home real."""

import asyncio
import contextlib
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest

from app.codex_importador import CodexNativo, CodexNativoErro


_SERVIDOR = r'''
import json
import os
import sys
import time

scenario = sys.argv[1]
args = sys.argv[2:]
assert args[:2] == ["-c", "project_root_markers=[]"]
args = args[2:]
assert os.getcwd() != os.environ["HOME"]
def send(value):
    print(json.dumps(value), flush=True)

def spawn_grandchild(role):
    import subprocess
    return subprocess.Popen([sys.executable, os.environ["HANGAR_T24_GRANDCHILD"], role],
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)

if scenario == "tree_shim":
    # Shim: o líder só repassa stdio a um app-server filho, como o executável do npm.
    import subprocess
    sys.exit(subprocess.call([sys.executable, __file__, "tree", *sys.argv[2:]]))
if scenario == "tree_cli" and args != ["app-server", "--stdio"]:
    spawn_grandchild("cli").wait()
    sys.exit(0)
if scenario.startswith("tree"):
    spawn_grandchild("app-server")

if args != ["app-server", "--stdio"]:
    if scenario == "cli_error":
        print("secret-token", file=sys.stderr)
        sys.exit(7)
    if scenario == "cli_auto_upgrade":
        print("Failed to upgrade marketplace `market`: installed marketplace `market` "
              "changed while auto-upgrade was in flight", file=sys.stderr)
        sys.exit(1)
    if scenario == "cli_invalid":
        print("secret-token")
        sys.exit(0)
    if scenario == "cli_errors_json":
        send({"errors": ["fatal: unable to access https://secret-token@example.invalid/repo"]})
        sys.exit(0)
    if scenario == "cli_timeout":
        time.sleep(30)
    if args == ["plugin", "list", "--json"]:
        send({"installed": [{"pluginId": "plugin@market", "version": "2.0.0",
              "marketplaceSource": {"sourceType": "git", "source": "https://example.invalid/repo.git"}}],
              "available": []} if scenario != "invalid_inventory" else {"installed": [None]})
        sys.exit(0)
    send({"args": args, "home": os.environ["HOME"],
          "profile": os.environ["USERPROFILE"], "codexHome": os.environ["CODEX_HOME"],
          "cwd": os.getcwd()})
    sys.exit(0)

initialized = False
for line in sys.stdin:
    msg = json.loads(line)
    method = msg["method"]
    if method == "initialized":
        initialized = True
        if scenario == "tree_leader_gone":
            sys.exit(0)
        continue
    rid = msg["id"]
    if method == "initialize" and scenario == "tree_init_hang":
        continue
    if method == "initialize":
        assert msg["params"]["capabilities"]["experimentalApi"] is True
        assert msg["params"]["clientInfo"]["name"] == "hangar"
        if scenario == "init_error":
            send({"id": rid, "error": {"code": -32601, "message": "secret-token"}})
        else:
            send({"id": rid, "result": {}})
        continue
    assert initialized
    if scenario == "eof":
        sys.exit(0)
    if scenario == "rpc_timeout":
        continue
    if scenario == "malformed":
        print("not JSON secret-token", flush=True)
        continue
    if scenario == "unsupported":
        send({"id": rid, "error": {"code": -32601, "message": "secret-token"}})
        continue
    if scenario == "readonly":
        send({"id": rid, "error": {"code": -32600, "message": "secret-token", "data": {
            "config_write_error_code": "configLayerReadonly", "config": "secret-token"}}})
        continue
    if method == "externalAgentConfig/import/readHistories":
        assert msg["params"] is None
        send({"id": rid, "result": {"data": [{"importId": "previous", "completedAtMs": 123,
              "successes": [{"itemType": "MCP_SERVER_CONFIG", "cwd": None, "source": "old", "target": "new"}],
              "failures": []}], "connectors": []} if scenario != "invalid_history" else {"data": {}}})
    elif method == "externalAgentConfig/detect":
        assert msg["params"] == {"includeHome": True, "cwds": [], "maxSessions": 0}
        if scenario.startswith("detect_problem_"):
            send({"id": rid, "result": {"items": [], scenario[len("detect_problem_"):]: ["secret-token"]}})
        else:
            send({"id": rid, "result": {"items": [{"itemType": "SKILLS", "description": "Skills"}]}})
    elif method == "externalAgentConfig/import":
        assert msg["params"]["migrationItems"][0]["itemType"] == "SKILLS"
        completed = {"importId": str(rid), "itemTypeResults": [{
            "itemType": "SKILLS", "successes": [{"itemType": "SKILLS", "target": "skill-a"}],
            "failures": [{"itemType": "SKILLS", "failureStage": "copy", "message": "Falha parcial"}],
        }]}
        # Um ID alheio não pode satisfazer a espera desta importação.
        send({"method": "externalAgentConfig/import/completed", "params": {
            "importId": "other", "itemTypeResults": []}})
        if scenario == "before":
            send({"method": "externalAgentConfig/import/completed", "params": completed})
        send({"id": rid, "result": {"importId": str(rid)}})
        if scenario == "import_eof":
            sys.exit(0)
        if scenario not in {"before", "import_timeout", "tree"}:
            send({"method": "externalAgentConfig/import/completed", "params": completed})
    elif method == "plugin/installed":
        send({"id": rid, "result": {"plugins": []}})
    else:
        raise AssertionError(method)
'''

# Descendente que segura o cwd e um arquivo dentro dele, avisa que está pronto pelo socket do
# teste e só sai quando o teste mandar (ou quando alguém o encerrar).
_GRANDCHILD = r'''
import json
import os
import socket
import sys

held = open(os.path.join(os.getcwd(), "grandchild.lock"), "w")
conn = socket.create_connection(("127.0.0.1", int(os.environ["HANGAR_T24_TREE_PORT"])))
conn.sendall((json.dumps({"pid": os.getpid(), "cwd": os.getcwd(), "role": sys.argv[1]}) + "\n").encode())
conn.recv(1)
'''


@pytest.fixture
def cliente(tmp_path, monkeypatch):
    script = tmp_path / "servidor.py"
    script.write_text(_SERVIDOR, encoding="utf-8")
    home = tmp_path / "home"
    home.mkdir()
    codex_home = home / ".codex"
    codex_home.mkdir()

    def criar(scenario="before", **kwargs):
        obj = CodexNativo(home, codex_home, timeout=kwargs.pop("timeout", 2),
                          close_timeout=0.05, **kwargs)
        monkeypatch.setattr(obj, "_comando", lambda: [sys.executable, str(script), scenario])
        return obj
    return criar


@pytest.mark.parametrize("scenario", ["before", "after"])
async def test_detectar_e_importar_preserva_falhas_e_notificacao_antecipada(cliente, scenario):
    obj = cliente(scenario)
    async with obj:
        proc = obj._proc
        items = await obj.detectar()
        assert items == [{"itemType": "SKILLS", "description": "Skills"}]
        result = await obj.importar(items)
        assert result["importId"] != "other"
        assert result["itemTypeResults"][0]["successes"][0]["target"] == "skill-a"
        assert result["itemTypeResults"][0]["failures"][0]["message"] == "Falha parcial"
        assert await obj.request("plugin/installed", {}) == {"plugins": []}
    assert proc.returncode == 0
    assert obj._reader_task is None
    assert not obj._pending
    await obj.close()


async def test_importacoes_concorrentes_nao_trocam_resultados(cliente):
    async with cliente() as obj:
        items = await obj.detectar()
        first, second = await asyncio.gather(obj.importar(items), obj.importar(items))
        assert first["importId"] != second["importId"]


@pytest.mark.parametrize("scenario, message", [
    ("unsupported", "atualize o Codex CLI"),
    ("eof", "encerrou a conexão"),
    ("malformed", "encerrou a conexão"),
    ("rpc_timeout", "tempo limite"),
])
async def test_falhas_da_requisicao_encerram_espera_sem_expor_segredos(cliente, scenario, message):
    async with cliente(scenario) as obj:
        obj.timeout = 0.1
        with pytest.raises(CodexNativoErro, match=message) as error:
            await obj.detectar()
        assert "secret-token" not in str(error.value)
        assert not obj._pending


@pytest.mark.parametrize("scenario, message", [
    ("import_timeout", "tempo limite"), ("import_eof", "encerrou antes"),
])
async def test_importacao_interrompida_nao_reporta_sucesso(cliente, scenario, message):
    async with cliente(scenario) as obj:
        obj.timeout = 0.1
        with pytest.raises(CodexNativoErro, match=message):
            await obj.importar([{"itemType": "SKILLS"}])
        assert obj._completions is None


async def test_erro_de_inicializacao_fecha_processo(cliente):
    obj = cliente("init_error")
    with pytest.raises(CodexNativoErro, match="atualize"):
        async with obj:
            pytest.fail("Inicialização inválida foi aceita")
    assert obj._proc is None
    assert obj._reader_task is None
    assert not obj._pending


async def test_cli_e_wrappers_usam_ambiente_e_argumentos_literais(cliente):
    obj = cliente()
    original_home = os.environ.get("HOME")
    result = await obj.instalar_plugin("plugin & literal@market")
    assert result["args"] == ["plugin", "add", "plugin & literal@market", "--json"]
    assert result["home"] == result["profile"] == str(obj.home)
    assert result["cwd"] != str(obj.home)
    assert not Path(result["cwd"]).exists()
    assert result["codexHome"] == str(obj.codex_home)
    assert os.environ.get("HOME") == original_home
    result = await obj.atualizar_marketplace("market")
    assert result["args"] == ["plugin", "marketplace", "upgrade", "market", "--json"]


async def test_marketplace_em_auto_upgrade_do_codex_nao_e_falha(cliente, caplog):
    obj = cliente("cli_auto_upgrade")
    result = await obj.atualizar_marketplace("market")
    assert result == {"selectedMarketplaces": ["market"], "upgradedRoots": [], "errors": []}
    from app import diag, log_paths
    assert not (log_paths.base() / "privado" / "codex" / diag.conta_id(obj.codex_home)).exists()
    assert "CLI Codex" not in caplog.text
    with pytest.raises(CodexNativoErro, match="código 7"):
        await cliente("cli_error").atualizar_marketplace("market")


@pytest.mark.parametrize("scenario, message", [
    ("cli_error", "código 7"), ("cli_invalid", "JSON válido"), ("cli_timeout", "tempo limite"),
])
async def test_cli_falha_sem_publicar_conteudo_sensivel(cliente, scenario, message):
    obj = cliente(scenario, timeout=0.1)
    with pytest.raises(CodexNativoErro, match=message) as error:
        await obj.cli(["plugin", "add", "plugin@market", "--json"])
    assert "secret-token" not in str(error.value)


@pytest.mark.parametrize("scenario", ["cli_error", "cli_invalid", "cli_errors_json"])
async def test_cli_guarda_diagnostico_privado_sem_tokens_no_log(cliente, scenario, caplog):
    obj = cliente(scenario)
    try:
        await obj.cli(["plugin", "marketplace", "upgrade", "market", "--json"])
    except CodexNativoErro:
        pass
    from app import diag, log_paths
    arquivos = list((log_paths.base() / "privado" / "codex" / diag.conta_id(obj.codex_home)).glob("*.log"))
    assert len(arquivos) == 1
    assert "secret-token" in arquivos[0].read_text()
    assert "secret-token" not in caplog.text
    assert str(arquivos[0]) in caplog.text
    if os.name != "nt":
        assert arquivos[0].stat().st_mode & 0o777 == 0o600


async def test_cancelamento_limpa_espera_e_processo(cliente):
    obj = cliente("import_timeout")
    async with obj:
        proc = obj._proc
        task = asyncio.create_task(obj.importar([{"itemType": "SKILLS"}]))
        await asyncio.sleep(0.02)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert obj._completions is None
        assert not obj._pending
    assert proc.returncode is not None


async def test_encerramento_recorre_a_kill_quando_necessario(tmp_path):
    obj = CodexNativo(tmp_path, tmp_path, close_timeout=0.01)
    proc = Mock(returncode=None, stdin=Mock())
    proc.wait = AsyncMock(side_effect=[TimeoutError, TimeoutError, 0])
    await obj._stop(proc)
    proc.stdin.close.assert_called_once()
    proc.terminate.assert_called_once()
    proc.kill.assert_called_once()
    assert proc.wait.await_count == 3


async def test_requisicao_sem_contexto_falha_imediatamente(tmp_path):
    with pytest.raises(CodexNativoErro, match="Abra o cliente"):
        await CodexNativo(tmp_path, tmp_path).detectar()


def test_binario_ausente_gera_erro_claro(tmp_path, monkeypatch):
    monkeypatch.setattr("app.codex_importador.shutil.which", lambda _: None)
    with pytest.raises(CodexNativoErro, match="não encontrado"):
        CodexNativo(tmp_path, tmp_path)._comando()


@pytest.mark.parametrize("nativo", [True, False])
def test_windows_resolve_shim_sem_interpretador_de_comandos(tmp_path, monkeypatch, nativo):
    from app import codex_importador

    shim = tmp_path / "codex.cmd"
    shim.write_text("@echo off", encoding="utf-8")
    monkeypatch.setattr(codex_importador, "os", SimpleNamespace(name="nt", environ={}))
    monkeypatch.setattr(codex_importador.shutil, "which", lambda _: str(shim))
    if nativo:
        binary = tmp_path / "codex.exe"
        binary.touch()
        esperado = [str(binary)]
    else:
        script = tmp_path / "node_modules" / "@openai" / "codex" / "bin" / "codex.js"
        script.parent.mkdir(parents=True)
        script.touch()
        node = tmp_path / "node.exe"
        node.touch()
        esperado = [str(node), str(script)]
    assert CodexNativo(tmp_path, tmp_path)._comando() == esperado


def test_windows_recusa_shim_sem_executavel_conhecido(tmp_path, monkeypatch):
    from app import codex_importador

    monkeypatch.setattr(codex_importador, "os", SimpleNamespace(name="nt", environ={}))
    monkeypatch.setattr(codex_importador.shutil, "which", lambda _: str(tmp_path / "codex.cmd"))
    with pytest.raises(CodexNativoErro, match="executável nativo"):
        CodexNativo(tmp_path, tmp_path)._comando()


async def test_close_desbloqueia_importacao_esperando_notificacao(cliente):
    obj = cliente("import_timeout")
    async with obj:
        task = asyncio.create_task(obj.importar([{"itemType": "SKILLS"}]))
        await asyncio.sleep(0.02)
        await obj.close()
        with pytest.raises(CodexNativoErro, match="encerrou"):
            await task


async def test_erro_expoe_apenas_codigo_e_discriminador_publico(cliente):
    async with cliente("readonly") as obj:
        with pytest.raises(CodexNativoErro) as error:
            await obj.request("config/batchWrite", {})
        assert error.value.code == -32600
        assert error.value.data == {"config_write_error_code": "configLayerReadonly"}
        assert "secret-token" not in str(error.value)


def test_home_temporaria_isola_xdg_herdado(cliente, monkeypatch):
    for name in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"]:
        monkeypatch.setenv(name, "diretorio-real")
    obj = cliente()
    env = obj._env()
    assert env["XDG_CONFIG_HOME"] == str(obj.home / ".config")
    assert env["XDG_DATA_HOME"] == str(obj.home / ".local" / "share")
    assert env["XDG_STATE_HOME"] == str(obj.home / ".local" / "state")
    assert env["XDG_CACHE_HOME"] == str(obj.home / ".cache")


def test_home_real_preserva_xdg_configurado(monkeypatch):
    from pathlib import Path

    monkeypatch.setenv("XDG_CONFIG_HOME", "configurado-pelo-usuario")
    obj = CodexNativo(Path.home(), Path.home() / ".codex")
    assert obj._env()["XDG_CONFIG_HOME"] == "configurado-pelo-usuario"


async def test_historico_preserva_origem_destino_e_importacao(cliente):
    async with cliente() as obj:
        historicos = await obj.historicos_importacao()
        assert historicos[0]["importId"] == "previous"
        assert historicos[0]["successes"] == [{
            "itemType": "MCP_SERVER_CONFIG", "cwd": None, "source": "old", "target": "new",
        }]


async def test_inventario_preserva_versao_e_origem_do_marketplace(cliente):
    plugins = await cliente().plugins_instalados()
    assert plugins == [{"pluginId": "plugin@market", "version": "2.0.0", "marketplaceSource": {
        "sourceType": "git", "source": "https://example.invalid/repo.git",
    }}]


async def test_historico_invalido_e_recusado(cliente):
    async with cliente("invalid_history") as obj:
        with pytest.raises(CodexNativoErro, match="histórico"):
            await obj.historicos_importacao()


async def test_inventario_invalido_e_recusado(cliente):
    with pytest.raises(CodexNativoErro, match="inventário"):
        await cliente("invalid_inventory").plugins_instalados()


def test_memoria_entra_por_linha_de_comando_e_so_quando_pedida(tmp_path):
    # O config.toml do stage é lido de volta como resultado da importação nativa: a flag tem de
    # viajar no argv, senão vira uma diferença nossa a conciliar.
    assert CodexNativo(tmp_path, tmp_path / ".codex")._config_memoria() == ()
    assert CodexNativo(tmp_path, tmp_path / ".codex", memoria=True)._config_memoria() == (
        "-c", "features.external_agent_memory_import=true")


@pytest.mark.parametrize("campo", ["sourceErrors", "errors", "warnings"])
async def test_detectar_nao_interpreta_falha_de_fonte_como_lista_vazia(cliente, campo):
    async with cliente("detect_problem_" + campo) as obj:
        with pytest.raises(CodexNativoErro, match="falhas ou avisos") as error:
            await obj.detectar()
        assert "secret-token" not in str(error.value)


_WINDOWS_TREE = pytest.mark.skipif(
    os.name != "nt", reason="a contenção da árvore própria do Codex é contrato do Windows")


def _alive(info: dict) -> bool:
    import psutil

    try:
        proc = psutil.Process(info["pid"])
        # Encerrado e ainda listado por causa de um handle alheio não segura mais nada.
        return (proc.create_time() == info["birth"] and proc.status() != psutil.STATUS_ZOMBIE
                and proc.num_threads() > 0)
    except psutil.NoSuchProcess:
        return False


def _assert_tree_gone(info: dict, phase: str) -> None:
    assert not _alive(info), (
        f"descendente próprio vivo após {phase}: pid={info['pid']} nascimento={info['birth']} "
        f"papel={info['role']}")
    assert not Path(info["cwd"]).exists(), f"pasta retida após {phase}: {info['cwd']}"


@pytest.fixture
async def process_tree(tmp_path, monkeypatch):
    import psutil

    script = tmp_path / "grandchild.py"
    script.write_text(_GRANDCHILD, encoding="utf-8")
    ready: asyncio.Queue = asyncio.Queue()
    writers: list[asyncio.StreamWriter] = []
    seen: list[dict] = []

    async def accept(reader, writer):
        info = json.loads(await reader.readline())
        info["birth"] = psutil.Process(info["pid"]).create_time()
        writers.append(writer)
        seen.append(info)
        await ready.put(info)

    server = await asyncio.start_server(accept, "127.0.0.1", 0)
    monkeypatch.setenv("HANGAR_T24_TREE_PORT", str(server.sockets[0].getsockname()[1]))
    monkeypatch.setenv("HANGAR_T24_GRANDCHILD", str(script))

    async def next_ready() -> dict:
        async with asyncio.timeout(20):
            return await ready.get()

    async def foreign() -> asyncio.subprocess.Process:
        alheio = tmp_path / "alheio"
        alheio.mkdir(exist_ok=True)
        return await asyncio.create_subprocess_exec(sys.executable, str(script), "alheio", cwd=alheio)

    try:
        yield SimpleNamespace(next_ready=next_ready, ready=ready, foreign=foreign)
    finally:
        # Ação explícita do teste: libera quem ainda estiver vivo (a baseline deixa o neto).
        for writer in writers:
            with contextlib.suppress(OSError):
                writer.write(b"x")
                await writer.drain()
            writer.close()
        server.close()
        await server.wait_closed()
        for info in seen:
            with contextlib.suppress(psutil.NoSuchProcess):
                await asyncio.to_thread(psutil.Process(info["pid"]).wait, 20)


@_WINDOWS_TREE
@pytest.mark.parametrize("scenario", ["tree", "tree_shim"])
async def test_process_tree_close_ends_own_descendant_before_removing_dir(cliente, process_tree, scenario):
    alheio = await process_tree.foreign()
    foreign = await process_tree.next_ready()
    obj = cliente(scenario)
    async with obj:
        own = await process_tree.next_ready()
        assert own["role"] == "app-server"
        assert _alive(own)
    _assert_tree_gone(own, "close")
    assert _alive(foreign), "processo alheio foi encerrado junto com a árvore do Codex"
    await obj.close()
    assert alheio.returncode is None


@_WINDOWS_TREE
async def test_process_tree_contained_when_leader_already_exited(cliente, process_tree):
    obj = cliente("tree_leader_gone")
    async with obj:
        own = await process_tree.next_ready()
        assert await obj._proc.wait() == 0
        assert _alive(own)
    _assert_tree_gone(own, "close com líder encerrado")


@_WINDOWS_TREE
async def test_process_tree_cancelled_initialize_ends_tree(cliente, process_tree):
    obj = cliente("tree_init_hang")
    opening = asyncio.create_task(obj.__aenter__())
    own = await process_tree.next_ready()
    opening.cancel()
    with pytest.raises(asyncio.CancelledError):
        await opening
    _assert_tree_gone(own, "cancelamento do initialize")
    assert obj._proc is None


@_WINDOWS_TREE
async def test_process_tree_cancelled_import_then_close_ends_tree(cliente, process_tree):
    obj = cliente("tree")
    async with obj:
        own = await process_tree.next_ready()
        notices = obj.subscribe("externalAgentConfig/import/completed")
        task = asyncio.create_task(obj.importar([{"itemType": "SKILLS"}]))
        # O servidor só avisa a conclusão alheia depois de receber a importação: ela está em curso.
        assert (await notices.get())["params"]["importId"] == "other"
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert _alive(own)
    _assert_tree_gone(own, "cancelamento da importação")


@_WINDOWS_TREE
@pytest.mark.parametrize("end", ["timeout", "cancel"])
async def test_process_tree_cli_timeout_or_cancel_ends_tree(cliente, process_tree, end):
    obj = cliente("tree_cli", timeout=30 if end == "cancel" else 1)
    task = asyncio.create_task(obj.cli(["plugin", "list", "--json"]))
    own = await process_tree.next_ready()
    assert own["role"] == "cli"
    if end == "timeout":
        with pytest.raises(CodexNativoErro, match="tempo limite"):
            await task
    else:
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
    _assert_tree_gone(own, f"{end} do cli")


@_WINDOWS_TREE
async def test_process_tree_concurrent_close_returns_only_after_tree_end(cliente, process_tree):
    obj = cliente("tree")
    await obj.__aenter__()
    own = await process_tree.next_ready()
    closes = {asyncio.create_task(obj.close()), asyncio.create_task(obj.close())}
    done, pending = await asyncio.wait(closes, return_when=asyncio.FIRST_COMPLETED)
    _assert_tree_gone(own, "primeiro close concorrente")
    await asyncio.gather(*done, *pending)
    await obj.close()
    _assert_tree_gone(own, "close repetido")


@_WINDOWS_TREE
async def test_process_tree_failed_assignment_ends_suspended_process(cliente, process_tree, monkeypatch):
    from app import runtime_process

    created = []
    original = asyncio.create_subprocess_exec

    async def capture(*args, **kwargs):
        proc = await original(*args, **kwargs)
        created.append(proc)
        return proc

    def refuse(self, proc):
        raise OSError(5, "acesso negado ao Job")

    monkeypatch.setattr(asyncio, "create_subprocess_exec", capture)
    monkeypatch.setattr(runtime_process.WindowsJob, "assign", refuse)
    obj = cliente("tree")
    with pytest.raises(CodexNativoErro, match="contenção"):
        await obj.__aenter__()
    assert len(created) == 1 and created[0].returncode is not None
    assert process_tree.ready.empty(), "processo sem contenção chegou a rodar"
    assert obj._proc is None and obj._work_dir is None


@_WINDOWS_TREE
async def test_process_tree_failed_membership_query_is_not_success(cliente, process_tree, monkeypatch):
    from app import runtime_process

    obj = cliente("tree")
    await obj.__aenter__()
    own = await process_tree.next_ready()
    original = runtime_process.WindowsJob.members

    def refuse(self):
        raise OSError(5, "consulta do Job negada")

    monkeypatch.setattr(runtime_process.WindowsJob, "members", refuse)
    with pytest.raises(CodexNativoErro, match="contenção"):
        await obj.close()
    assert Path(own["cwd"]).exists()
    monkeypatch.setattr(runtime_process.WindowsJob, "members", original)
    await obj.close()
    _assert_tree_gone(own, "close após consulta recusada")


async def test_process_tree_cli_second_cancel_and_concurrent_close_wait_for_cleanup(
        cliente, process_tree, monkeypatch):
    import psutil

    alheio = await process_tree.foreign()
    foreign = await process_tree.next_ready()
    obj = cliente("tree_cli", timeout=30)
    spawned = []
    original_spawn, original_finish = obj._spawn, obj._finish
    cleaning, release = asyncio.Event(), asyncio.Event()

    async def capture(*args, **kwargs):
        pair = await original_spawn(*args, **kwargs)
        spawned.append(pair)
        return pair

    async def gated(proc, tree):
        # Barreira só de instrumentação: a limpeza real roda inteira depois da liberação.
        if proc is not None and not cleaning.is_set():
            cleaning.set()
            await release.wait()
        await original_finish(proc, tree)

    monkeypatch.setattr(obj, "_spawn", capture)
    monkeypatch.setattr(obj, "_finish", gated)
    task = asyncio.create_task(obj.cli(["plugin", "list", "--json"]))
    closing = None
    try:
        own = await process_tree.next_ready()
        leader = {"pid": spawned[0][0].pid, "birth": psutil.Process(spawned[0][0].pid).create_time(),
                  "role": "líder", "cwd": own["cwd"]}
        task.cancel()
        async with asyncio.timeout(20):
            await cleaning.wait()
        task.cancel()
        closing = asyncio.create_task(obj.close())
        done, _ = await asyncio.wait({task, closing}, timeout=1)
        assert not done, "cli ou close voltou com a limpeza própria ainda pendente"
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        await closing
        assert not _alive(leader), f"líder próprio vivo após close: pid={leader['pid']}"
        assert not Path(own["cwd"]).exists(), f"pasta retida após close: {own['cwd']}"
        if os.name == "nt":
            _assert_tree_gone(own, "segundo cancelamento do cli e close concorrente")
        assert _alive(foreign) and alheio.returncode is None
    finally:
        release.set()
        for pending in (task, closing):
            if pending is not None and not pending.done():
                pending.cancel()
            if pending is not None:
                with contextlib.suppress(asyncio.CancelledError, CodexNativoErro):
                    await pending


@_WINDOWS_TREE
async def test_process_tree_cli_failed_membership_query_is_retried_by_close(cliente, process_tree, monkeypatch):
    from app import runtime_process

    alheio = await process_tree.foreign()
    foreign = await process_tree.next_ready()
    obj = cliente("tree_cli", timeout=30)
    task = asyncio.create_task(obj.cli(["plugin", "list", "--json"]))
    own = await process_tree.next_ready()
    original = runtime_process.WindowsJob.members

    def refuse(self):
        raise OSError(5, "consulta do Job negada")

    monkeypatch.setattr(runtime_process.WindowsJob, "members", refuse)
    task.cancel()
    with pytest.raises(CodexNativoErro, match="contenção"):
        await task
    # Antes da nova tentativa: a árvore e a pasta continuam com o cliente, não somem nem são soltas.
    assert _alive(own) and Path(own["cwd"]).exists()
    monkeypatch.setattr(runtime_process.WindowsJob, "members", original)
    await obj.close()
    _assert_tree_gone(own, "close após consulta recusada no cli")
    assert _alive(foreign) and alheio.returncode is None


async def test_falha_de_limpeza_nao_esconde_o_erro_do_comando(cliente, monkeypatch):
    from app import codex_importador

    obj = cliente("cli_timeout", timeout=0.1)
    calls = []
    original = codex_importador.shutil.rmtree

    def locked(path):
        # No Windows a pasta ainda presa pelo filho recusa a remoção.
        calls.append(path)
        if len(calls) == 1:
            raise PermissionError(13, "em uso")
        original(path)

    monkeypatch.setattr(codex_importador.shutil, "rmtree", locked)
    with pytest.raises(CodexNativoErro, match="tempo limite"):
        await obj.cli(["plugin", "add", "plugin@market", "--json"])
    # A limpeza continua registrada: o encerramento tenta de novo e remove a pasta.
    await obj.close()
    assert len(calls) == 2
    assert not os.path.exists(calls[0])

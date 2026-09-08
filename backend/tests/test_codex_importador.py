"""Contrato do importador nativo com processos locais sem acesso à home real."""

import asyncio
import os
import sys
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
def send(value):
    print(json.dumps(value), flush=True)

if args != ["app-server", "--stdio"]:
    if scenario == "cli_error":
        print("secret-token", file=sys.stderr)
        sys.exit(7)
    if scenario == "cli_invalid":
        print("secret-token")
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
        continue
    rid = msg["id"]
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
        if scenario not in {"before", "import_timeout"}:
            send({"method": "externalAgentConfig/import/completed", "params": completed})
    elif method == "plugin/installed":
        send({"id": rid, "result": {"plugins": []}})
    else:
        raise AssertionError(method)
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
    assert result["home"] == result["profile"] == result["cwd"] == str(obj.home)
    assert result["codexHome"] == str(obj.codex_home)
    assert os.environ.get("HOME") == original_home
    result = await obj.atualizar_marketplace("market")
    assert result["args"] == ["plugin", "marketplace", "upgrade", "market", "--json"]


@pytest.mark.parametrize("scenario, message", [
    ("cli_error", "código 7"), ("cli_invalid", "JSON válido"), ("cli_timeout", "tempo limite"),
])
async def test_cli_falha_sem_publicar_conteudo_sensivel(cliente, scenario, message):
    obj = cliente(scenario, timeout=0.1)
    with pytest.raises(CodexNativoErro, match=message) as error:
        await obj.cli(["plugin", "add", "plugin@market", "--json"])
    assert "secret-token" not in str(error.value)


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


@pytest.mark.parametrize("campo", ["sourceErrors", "errors", "warnings"])
async def test_detectar_nao_interpreta_falha_de_fonte_como_lista_vazia(cliente, campo):
    async with cliente("detect_problem_" + campo) as obj:
        with pytest.raises(CodexNativoErro, match="falhas ou avisos") as error:
            await obj.detectar()
        assert "secret-token" not in str(error.value)

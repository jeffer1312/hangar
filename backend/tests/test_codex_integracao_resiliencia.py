"""Regressões de proveniência, fontes inválidas e cancelamento do reconciliador."""
import asyncio
import json
import os
from pathlib import Path
import shutil
import threading
from unittest.mock import AsyncMock

import pytest

from app import codex_integracao, skill_bridge
from app.codex_arquivos import exclusivo, hash_bytes
from app.codex_fragmentos import reconciliar_arquivos
from app.codex_integracao import IntegracaoCodex, _snapshot


class NativoVazio:
    def __init__(self, *args):
        pass

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        pass


@pytest.fixture
def home(tmp_path, monkeypatch):
    raiz = tmp_path / "home"
    (raiz / ".claude").mkdir(parents=True)
    (raiz / ".codex").mkdir()
    (raiz / ".claude/settings.json").write_text("{}", encoding="utf-8")
    monkeypatch.setattr(skill_bridge, "_REPO", tmp_path / "repo")
    monkeypatch.setattr(codex_integracao, "_REPO", tmp_path / "repo")
    return raiz


def _servico_isolado(home, monkeypatch):
    service = IntegracaoCodex(home, home / ".codex", nativo=NativoVazio)
    monkeypatch.setattr(service, "_instrucoes", lambda: None)
    monkeypatch.setattr(service, "_hooks", lambda *args: None)
    monkeypatch.setattr(service, "_config", AsyncMock())
    monkeypatch.setattr(service, "_plugins", AsyncMock())
    monkeypatch.setattr(service, "_fragmentos", AsyncMock())
    monkeypatch.setattr(service, "_conferir_confianca", AsyncMock())
    return service


@pytest.mark.parametrize("valor", ["true", "false", 1, 0, None, [], {}])
async def test_enabled_plugins_valor_invalido_nao_desabilita_adotado(home, monkeypatch, valor):
    (home / ".claude/settings.json").write_text(json.dumps({"enabledPlugins": {"p@market": valor}}))
    service = IntegracaoCodex(home, home / ".codex", nativo=NativoVazio)
    service._estado = _snapshot()
    habilitar = AsyncMock()
    monkeypatch.setattr(service, "_habilitar_plugins", habilitar)
    native = type("Native", (), {"detectar": AsyncMock(return_value=[]),
                                  "plugins_instalados": AsyncMock(return_value=[])})()
    registro = {"plugins": {"p@market": {"path": str(home / "plugin"), "versao": "1", "origem": "market"}}}
    with pytest.raises(ValueError, match="enabledPlugins"):
        await service._plugins(native, set(), registro, False)
    habilitar.assert_not_awaited()
    assert "p@market" in registro["plugins"]


def test_fingerprint_detecta_edicao_em_skill_ligada_por_symlink(home):
    source = home / "repositorio-externo/skill"
    source.mkdir(parents=True)
    (source / "SKILL.md").write_text("versão A", encoding="utf-8")
    ponte = home / ".claude/skills/probe"
    ponte.parent.mkdir(parents=True)
    try:
        ponte.symlink_to(source, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"Symlink indisponível: {exc}")
    service = IntegracaoCodex(home, home / ".codex")
    antes = service.fingerprint(fontes=True)
    (source / "SKILL.md").write_text("versão B mais recente", encoding="utf-8")
    assert service.fingerprint(fontes=True) != antes


@pytest.mark.parametrize("habilitado,confirmado", [(False, True), (True, False)])
def test_plugin_inativo_ou_bloqueado_nao_retira_ponte(home, habilitado, confirmado):
    source = home / ".claude/plugins/cache/market/p/1/skills/probe"
    native = home / ".codex/plugins/cache/market/p/1/skills/probe"
    for path in (source, native):
        path.mkdir(parents=True)
        (path / "SKILL.md").write_text("---\nname: probe\ndescription: Teste\n---\nTeste\n", encoding="utf-8")
    ponte = home / ".codex/skills/probe"
    ponte.parent.mkdir(parents=True)
    try:
        ponte.symlink_to(source, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"Symlink indisponível: {exc}")
    (home / ".codex/config.toml").write_text(f'[plugins."p@market"]\nenabled={str(habilitado).lower()}\n')
    service = IntegracaoCodex(home, home / ".codex")
    service._estado = _snapshot()
    service._plugins_confirmados = {"p@market"} if confirmado else set()
    registro = {"plugins": {"p@market": {"path": str(native.parents[1]), "versao": "1", "origem": "market"}}}
    service._skills(registro)
    assert ponte.is_symlink()
    assert ponte.resolve() == source
    assert registro["skills"]["probe"]["mode"] == "symlink"


async def test_cancelamento_persiste_proveniencia_de_artefato_ja_materializado(home, monkeypatch):
    service = _servico_isolado(home, monkeypatch)
    pronto = asyncio.Event()
    path = home / ".codex/agents/probe.toml"
    async def fragmentos(native, settings, registro):
        registro["artefatos"], _ = reconciliar_arquivos({path: b"v1"}, {}, service.backups)
        pronto.set()
        await asyncio.Future()
    monkeypatch.setattr(service, "_fragmentos", fragmentos)
    task = asyncio.create_task(service.reconciliar())
    try:
        await asyncio.wait_for(pronto.wait(), 5)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        estado = json.loads((service.raiz / "estado.json").read_text())
        assert estado["artefatos"][str(path)]["hash"] == hash_bytes(b"v1")
        assert estado["status"]["estado"] == "ocioso"
        novo, avisos = reconciliar_arquivos({path: b"v2"}, estado["artefatos"], service.backups)
        assert avisos == []
        assert novo[str(path)]["hash"] == hash_bytes(b"v2")
        assert path.read_bytes() == b"v2"
    finally:
        if not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)


async def test_cancelamento_aguarda_worker_antes_de_salvar_e_liberar_lock(home, monkeypatch):
    service = _servico_isolado(home, monkeypatch)
    loop = asyncio.get_running_loop()
    iniciou = asyncio.Event()
    liberar = threading.Event()
    concluiu = threading.Event()
    path = home / ".codex/skills/probe/SKILL.md"
    def skills(registro):
        loop.call_soon_threadsafe(iniciou.set)
        if not liberar.wait(5):
            raise RuntimeError("Teste não liberou o worker")
        path.parent.mkdir(parents=True)
        path.write_bytes(b"fonte")
        registro["skills"] = {"probe": {"path": str(path.parent), "mode": "copy",
                                        "files": {"SKILL.md": hash_bytes(b"fonte")}}}
        concluiu.set()
    monkeypatch.setattr(service, "_skills", skills)
    task = asyncio.create_task(service.reconciliar())
    lock_liberado = asyncio.Event()
    async def concorrente():
        async with exclusivo(service.codex_home / ".hangar-integracao.lock"):
            lock_liberado.set()
    disputa = None
    try:
        await asyncio.wait_for(iniciou.wait(), 5)
        task.cancel()
        disputa = asyncio.create_task(concorrente())
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(lock_liberado.wait(), 0.05)
        assert not task.done()
        assert not concluiu.is_set()
        liberar.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 5)
        await asyncio.wait_for(disputa, 5)
        assert concluiu.is_set()
        estado = json.loads((service.raiz / "estado.json").read_text())
        assert estado["skills"]["probe"]["files"]["SKILL.md"] == hash_bytes(b"fonte")
        assert path.read_bytes() == b"fonte"
    finally:
        liberar.set()
        await asyncio.gather(task, return_exceptions=True)
        if disputa is not None:
            await asyncio.gather(disputa, return_exceptions=True)


real_codex = pytest.mark.skipif(
    os.environ.get("RUN_CODEX_INTEGRATION") != "1" or not shutil.which("codex"),
    reason="Exige RUN_CODEX_INTEGRATION=1 e Codex CLI; somente HOME temporária",
)


@pytest.mark.integration
@real_codex
@pytest.mark.parametrize("tipo", ["hooks", "mcp", "agent"])
async def test_fonte_invalida_nao_vira_remocao_no_importador_real(home, monkeypatch, tipo):
    settings = home / ".claude/settings.json"
    settings.write_text(json.dumps({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
        {"type": "command", "command": "echo guard"},
    ]}]}}))
    mcp = home / ".claude.json"
    mcp.write_text(json.dumps({"mcpServers": {"probe": {"command": "echo"}}}))
    agent = home / ".claude/agents/probe.md"
    agent.parent.mkdir()
    agent.write_text("---\nname: probe\ndescription: Teste\n---\nTexto\n", encoding="utf-8")
    service = IntegracaoCodex(home, home / ".codex")
    monkeypatch.setattr(service, "_plugins", AsyncMock())
    monkeypatch.setattr(service, "_skills", lambda registro: None)
    primeira = await service.reconciliar()
    assert primeira["estado"] == "ok", primeira
    protegidos = [home / ".codex/hooks.json", home / ".codex/config.toml", home / ".codex/agents/probe.toml"]
    antes = {path: path.read_bytes() for path in protegidos}
    if tipo == "hooks":
        settings.write_text(json.dumps({"hooks": []}))
    elif tipo == "mcp":
        mcp.write_text(json.dumps({"mcpServers": []}))
    else:
        agent.write_text("---\nname: [invalido\n---\nTexto\n", encoding="utf-8")
    segunda = await service.reconciliar()
    if tipo == "agent":
        assert any(a["codigo"] == "aviso_ignorados" for a in segunda["avisos"]), segunda
    else:
        assert segunda["estado"] in {"erro", "parcial"}, segunda
    assert {path: path.read_bytes() for path in protegidos} == antes

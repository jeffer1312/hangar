import asyncio
import json
import os
import shutil
import time
import tomllib
from pathlib import Path

import pytest

from app.codex_integracao import IntegracaoCodex


def _home(tmp_path):
    (tmp_path / ".claude").mkdir()
    (tmp_path / ".codex").mkdir()
    (tmp_path / ".claude/settings.json").write_text('{"enabledPlugins": {}}')
    (tmp_path / ".claude/CLAUDE.md").write_text("Instruções globais\n")
    return tmp_path


def test_status_nao_cria_arquivos_nem_roda_binario(tmp_path):
    service = IntegracaoCodex(tmp_path, tmp_path / ".codex", binario="nao-existe")
    assert service.status()["estado"] == "ocioso"
    assert not list(tmp_path.iterdir())


async def test_iniciar_coalesce_e_nao_espera_execucao(tmp_path, monkeypatch):
    service = IntegracaoCodex(tmp_path, tmp_path / ".codex")
    gate = asyncio.Event()
    chamadas = []

    async def rodada(motivo, forcar):
        chamadas.append((motivo, forcar))
        await gate.wait()

    monkeypatch.setattr(service, "reconciliar", rodada)
    assert (await service.iniciar())["estado"] == "executando"
    await service.iniciar()
    await asyncio.sleep(0)
    assert chamadas == [("manual", True)]
    await service.fechar()
    assert service._task.cancelled()


async def test_settings_invalidos_nao_desabilitam_plugin(tmp_path):
    home = _home(tmp_path)
    (home / ".claude/settings.json").write_text('{"enabledPlugins": []}')
    config = home / ".codex/config.toml"
    config.write_text('[plugins."meu@marketplace"]\nenabled=true\n')
    service = IntegracaoCodex(home, home / ".codex", nativo=object)
    result = await service.reconciliar()
    assert result["estado"] == "erro"
    assert config.read_text() == '[plugins."meu@marketplace"]\nenabled=true\n'


async def test_sem_claude_e_informativo_e_nao_cria_codex(tmp_path):
    service = IntegracaoCodex(tmp_path, tmp_path / ".codex")
    result = await service.reconciliar()
    assert result["estado"] == "indisponivel"
    assert not list(tmp_path.iterdir())


def test_reinicio_nao_reporta_operacao_fantasma(tmp_path):
    service = IntegracaoCodex(tmp_path, tmp_path / ".codex")
    service.raiz.mkdir(parents=True)
    (service.raiz / "estado.json").write_text(json.dumps({"status": {"estado": "executando"}}))
    assert service.status()["estado"] == "ocioso"


real_codex = pytest.mark.skipif(os.environ.get("RUN_CODEX_INTEGRATION") != "1" or not shutil.which("codex"),
                                reason="Exige RUN_CODEX_INTEGRATION=1 e Codex CLI; usa somente HOME temporário")


@pytest.mark.integration
@real_codex
async def test_nativo_reconcilia_instalacao_existente_e_repete_sem_reescrever(tmp_path):
    home = _home(tmp_path)
    settings = {"hooks": {
        "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]}],
        "SessionEnd": [{"hooks": [{"type": "command", "command": "echo fim", "timeout": 5}]}],
    }}
    (home / ".claude/settings.json").write_text(json.dumps(settings))
    live = {"hooks": {"PreToolUse": [
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]},
        {"hooks": [{"type": "command", "command": "echo exclusivo"}]},
    ]}}
    (home / ".codex/hooks.json").write_text(json.dumps(live))
    config = home / ".codex/config.toml"
    config.write_text('# pessoal\nmodel="modelo-pessoal"\nproject_doc_fallback_filenames=["TEAM.md"]\n')
    instructions = home / ".codex/AGENTS.md"
    instructions.write_text("Instruções exclusivas do Codex\n")
    service = IntegracaoCodex(home, home / ".codex")
    primeira = await service.reconciliar()
    assert primeira["estado"] == "ok", primeira
    cfg = tomllib.loads(config.read_text())
    assert cfg["model"] == "modelo-pessoal"
    assert cfg["project_doc_fallback_filenames"] == ["TEAM.md", "CLAUDE.md", "CLAUDE.MD"]
    assert "# pessoal" in config.read_text()
    assert "Instruções exclusivas do Codex" in instructions.read_text()
    hooks = json.loads((home / ".codex/hooks.json").read_text())["hooks"]
    comandos = [e["command"] for g in hooks["PreToolUse"] for e in g["hooks"]]
    assert sum("codex-hook-allow.py" in c for c in comandos) == 1
    assert "echo exclusivo" in comandos
    assert hooks["SessionEnd"][0]["hooks"][0]["timeout"] == 3
    arquivos = [config, instructions, home / ".codex/hooks.json"]
    antes = {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in arquivos}
    segunda = await service.reconciliar()
    assert segunda["estado"] == "ok", segunda
    assert {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in arquivos} == antes
    assert segunda["confianca_pendente"] is True


@pytest.mark.integration
@real_codex
async def test_nativo_reaplica_mcp_e_agente_sem_alterar_exclusivos(tmp_path):
    home = _home(tmp_path)
    (home / ".claude.json").write_text(json.dumps({"mcpServers": {"local-probe": {"command": "echo", "args": ["primeiro"]}}}))
    (home / ".claude/agents").mkdir()
    (home / ".claude/agents/probe.md").write_text('---\nname: probe\ndescription: Agente de teste\n---\nPrimeira instrução\n')
    (home / ".claude/commands").mkdir()
    (home / ".claude/commands/testar.md").write_text('---\ndescription: Comando de teste\n---\nTeste um\n')
    cfg = home / ".codex/config.toml"
    cfg.write_text('[mcp_servers.exclusivo]\ncommand="echo"\nargs=["meu"]\n')
    service = IntegracaoCodex(home, home / ".codex")
    primeiro = await service.reconciliar()
    assert primeiro["estado"] == "ok", primeiro
    parsed = tomllib.loads(cfg.read_text())
    assert parsed["mcp_servers"]["exclusivo"]["args"] == ["meu"]
    assert parsed["mcp_servers"]["local-probe"]["args"] == ["primeiro"]
    assert str(service.raiz) not in cfg.read_text()
    (home / ".claude.json").write_text(json.dumps({"mcpServers": {"local-probe": {"command": "echo", "args": ["segundo"]}}}))
    segundo = await service.reconciliar()
    assert segundo["estado"] == "ok", segundo
    assert tomllib.loads(cfg.read_text())["mcp_servers"]["local-probe"]["args"] == ["segundo"]
    assert any("Primeira instrução" in p.read_text() for p in (home / ".codex/agents").glob("*.toml"))


@pytest.mark.integration
@real_codex
async def test_adota_importacao_nativa_anterior_remove_gerenciados_preserva_colisao(tmp_path):
    from app.codex_importador import CodexNativo
    home = _home(tmp_path)
    source = home / '.claude.json'
    source.write_text(json.dumps({'mcpServers': {'adotado': {'command': 'echo', 'args': ['antes']}}}))
    async with CodexNativo(home, home / '.codex') as native:
        await native.importar([i for i in await native.detectar() if i['itemType'] == 'MCP_SERVER_CONFIG'])
    cfg = home / '.codex/config.toml'
    with cfg.open('a') as f:
        f.write('\n[mcp_servers.colisao]\ncommand="echo"\nargs=["particular"]\n')
    source.write_text(json.dumps({'mcpServers': {
        'adotado': {'command': 'echo', 'args': ['depois']},
        'colisao': {'command': 'echo', 'args': ['claude']},
    }}))
    service = IntegracaoCodex(home, home / '.codex')
    result = await service.reconciliar()
    assert result['estado'] == 'ok', result
    data = tomllib.loads(cfg.read_text())['mcp_servers']
    assert data['adotado']['args'] == ['depois']
    assert data['colisao']['args'] == ['particular']
    assert any(a['params'].get('nome') == 'colisao' for a in result['avisos'])
    source.write_text('{"mcpServers": {}}')
    result = await service.reconciliar()
    assert result['estado'] == 'ok', result
    data = tomllib.loads(cfg.read_text())['mcp_servers']
    assert 'adotado' not in data
    assert data['colisao']['args'] == ['particular']


@pytest.mark.integration
@real_codex
async def test_config_rele_snapshot_apos_importacao_externa(tmp_path, monkeypatch):
    from app import codex_integracao as modulo
    home = _home(tmp_path)
    cfg = home / '.codex/config.toml'
    cfg.write_text('# particular\n')
    service = IntegracaoCodex(home, home / '.codex')
    service.raiz.mkdir(parents=True)
    service._estado = service.status()
    original = modulo.gravar
    alteracoes = []

    def troca(path, data, esperado, backups=None):
        if path == cfg and not alteracoes:
            alteracoes.append(True)
            cfg.write_text('# alterado pelo Desktop\nmodel="externo"\n')
        return original(path, data, esperado, backups)

    monkeypatch.setattr(modulo, 'gravar', troca)
    await service._config(None, {}, {})
    assert alteracoes == [True]
    data = tomllib.loads(cfg.read_text())
    assert data['model'] == 'externo'
    assert data['project_doc_fallback_filenames'] == ['CLAUDE.md', 'CLAUDE.MD']
    assert '# alterado pelo Desktop' in cfg.read_text()


@pytest.mark.integration
@real_codex
async def test_fonte_alterada_durante_importacao_e_relida(tmp_path):
    from app.codex_importador import CodexNativo
    home = _home(tmp_path)
    source = home / '.claude.json'
    source.write_text(json.dumps({'mcpServers': {'teste': {'command': 'echo', 'args': ['velho']}}}))
    mudou = []

    class ComAlteracaoExterna(CodexNativo):
        async def importar(self, itens):
            result = await super().importar(itens)
            if self.home != home and not mudou:
                mudou.append(True)
                source.write_text(json.dumps({'mcpServers': {'teste': {'command': 'echo', 'args': ['novo']}}}))
            return result

    service = IntegracaoCodex(home, home / '.codex', nativo=ComAlteracaoExterna)
    result = await service.reconciliar()
    assert result['estado'] == 'ok', result
    assert mudou
    assert tomllib.loads((home / '.codex/config.toml').read_text())['mcp_servers']['teste']['args'] == ['novo']
    assert json.loads((service.raiz / 'estado.json').read_text())['fingerprint'] is None


async def test_falha_marketplace_permanece_visivel_ate_nova_tentativa(tmp_path, monkeypatch):
    from app.codex_importador import CodexNativoErro
    home = _home(tmp_path)
    plugin = home / '.codex/plugins/cache/mercado/plugin/1'
    plugin.mkdir(parents=True)
    (home / '.claude/settings.json').write_text('{"enabledPlugins":{"plugin@mercado":true}}')
    (home / '.claude/plugins').mkdir()
    (home / '.claude/plugins/known_marketplaces.json').write_text(json.dumps({
        'mercado': {'source': {'source': 'github', 'repo': 'exemplo/mercado'}},
    }))
    (home / '.codex/config.toml').write_text('[marketplaces.mercado]\nsource_type="git"\nsource="https://github.com/exemplo/mercado.git"\n')
    service = IntegracaoCodex(home, home / '.codex')
    service.raiz.mkdir(parents=True)
    registro = {}
    chamadas = []

    class Native:
        async def detectar(self): return []
        async def plugins_instalados(self): return [{'pluginId': 'plugin@mercado', 'version': '1'}]
        async def atualizar_marketplace(self, nome):
            chamadas.append(nome)
            raise CodexNativoErro('Falha simulada de rede')
        async def instalar_plugin(self, nome): return {'installedPath': str(plugin), 'version': '1'}

    async def habilitar(*args): pass
    monkeypatch.setattr(service, '_habilitar_plugins', habilitar)
    for rodada in range(2):
        service._estado = {**service.status(), 'estado': 'executando', 'erros': []}
        await service._plugins(Native(), {'plugin@mercado'}, registro, False)
        assert service.status()['erros']
        assert registro['marketplaces_pendentes'] == ['mercado']
    assert chamadas == ['mercado']


def test_persona_antiga_continua_ligada_a_fonte_nativa(tmp_path):
    home = _home(tmp_path)
    source = home / '.claude/CLAUDE.md'
    source.write_text('Texto global que deve permanecer somente na fonte')
    target = home / '.codex/AGENTS.md'
    try:
        target.symlink_to(source)
    except OSError:
        pytest.skip('Symlink indisponível nesta máquina')
    service = IntegracaoCodex(home, home / '.codex')
    service._instrucoes()
    assert target.is_symlink()
    assert (home / '.codex/AGENTS.override.md').read_text() == source.read_text()
    assert source.read_text() == 'Texto global que deve permanecer somente na fonte'

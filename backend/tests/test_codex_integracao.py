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


def test_memoria_leva_um_transcrito_por_projeto(tmp_path):
    from app.codex_integracao import copiar_memorias
    origem = tmp_path / "projects"
    projeto = origem / "-home-alguem-repo"
    (projeto / "memory").mkdir(parents=True)
    (projeto / "memory" / "MEMORY.md").write_text("indice")
    # O menor de todos não tem conversa: o detector ignoraria o projeto, então ele não serve.
    (projeto / "so-metadado.jsonl").write_text('{"type":"mode"}\n')
    (projeto / "menor.jsonl").write_text('{"type":"user"}\n' + "x" * 200)
    (projeto / "grande.jsonl").write_text('{"type":"user"}\n' + "x" * 5000)
    (origem / "-sem-memoria").mkdir()
    (origem / "-sem-memoria" / "sessao.jsonl").write_text('{"type":"user"}\n')

    # Memória cuja conversa o Claude já apagou: o Codex não reconheceria o projeto. Fica de fora
    # calada — é regra dele, acontece o tempo todo, e não há o que a pessoa faça a respeito.
    (origem / "-orfao" / "memory").mkdir(parents=True)
    (origem / "-orfao" / "memory" / "nota.md").write_text("sem transcrito")
    (origem / "-vazio" / "memory").mkdir(parents=True)

    destino = tmp_path / "stage"
    erros = copiar_memorias(origem, destino)

    assert (destino / "-home-alguem-repo" / "menor.jsonl").exists()
    assert not (destino / "-home-alguem-repo" / "grande.jsonl").exists()
    assert not (destino / "-home-alguem-repo" / "so-metadado.jsonl").exists()
    assert (destino / "-home-alguem-repo" / "memory" / "MEMORY.md").read_text() == "indice"
    assert not (destino / "-sem-memoria").exists()
    assert erros == [], "só erro de leitura vira aviso"
    assert not (destino / "-orfao").exists()


async def test_memoria_copiada_que_o_codex_nao_reconhece_vira_aviso(tmp_path, monkeypatch):
    """Copiar não é ser reconhecido. O detector exige uma conversa ao lado da memória e não diz o
    quanto — sem esta conferência, a memória some da importação sem nenhum sinal."""
    from unittest.mock import AsyncMock
    from app import codex_integracao
    from app.codex_importador import CodexNativoErro
    home = _home(tmp_path)
    (home / ".claude/settings.json").write_text('{"enabledPlugins": {}, "hooks": {}, "env": {}}')
    for nome in ("-visto", "-ignorado"):
        (home / f".claude/projects/{nome}/memory").mkdir(parents=True)
        (home / f".claude/projects/{nome}/memory/MEMORY.md").write_text("indice")
        (home / f".claude/projects/{nome}/sessao.jsonl").write_text('{"type":"user"}\n')
    monkeypatch.setattr(codex_integracao, "memoria_ligada", lambda: True)

    class Importer:
        def __init__(self, stage, cx, binario, **kwargs):
            self.stage, self.cx = stage, cx
        async def __aenter__(self): return self
        async def __aexit__(self, *a): pass
        async def detectar(self):
            # O Codex enxergou só um dos dois projetos copiados.
            return [{"itemType": "MEMORY", "details": {"memory": ["-visto"]}}]
        async def importar(self, itens):
            (self.cx / "hooks.json").write_text('{"hooks": {}}')
            return {"itemTypeResults": []}
        async def historicos_importacao(self):
            raise CodexNativoErro("sem histórico")

    service = IntegracaoCodex(home, home / ".codex", nativo=Importer)
    service._estado = codex_integracao._snapshot()
    service.raiz.mkdir(parents=True)
    monkeypatch.setattr(service, "_config", AsyncMock())

    await service._fragmentos(Importer(None, None, None), {}, {})

    avisos = " ".join(service._estado["avisos"])
    assert "-ignorado" in avisos and "-visto" not in avisos


async def test_memoria_recusada_nao_derruba_o_resto_da_integracao(tmp_path, monkeypatch):
    """A memória vai numa chamada à parte justamente para isso: o Codex recusando uma memória não
    pode custar hooks, skills e MCP, que não dependem dela."""
    from unittest.mock import AsyncMock
    from app import codex_integracao
    from app.codex_importador import CodexNativoErro
    home = _home(tmp_path)
    (home / ".claude/settings.json").write_text('{"enabledPlugins": {}, "hooks": {}, "env": {}}')
    projeto = home / ".claude/projects/-um-repo"
    (projeto / "memory").mkdir(parents=True)
    (projeto / "memory/MEMORY.md").write_text("indice")
    (projeto / "sessao.jsonl").write_text('{"type":"user"}\n')
    monkeypatch.setattr(codex_integracao, "memoria_ligada", lambda: True)

    lotes = []

    class Importer:
        def __init__(self, stage, cx, binario, **kwargs):
            self.stage, self.cx = stage, cx
        async def __aenter__(self): return self
        async def __aexit__(self, *a): pass
        async def detectar(self):
            return [{"itemType": "HOOKS", "details": {}}, {"itemType": "MEMORY", "details": {}}]
        async def importar(self, itens):
            lotes.append(sorted({i["itemType"] for i in itens}))
            if any(i["itemType"] == "MEMORY" for i in itens):
                raise CodexNativoErro("o Codex recusou a memória")
            (self.cx / "hooks.json").write_text('{"hooks": {}}')
            return {"itemTypeResults": []}
        async def historicos_importacao(self):
            raise CodexNativoErro("sem histórico")

    service = IntegracaoCodex(home, home / ".codex", nativo=Importer)
    service._estado = codex_integracao._snapshot()
    service.raiz.mkdir(parents=True)
    monkeypatch.setattr(service, "_config", AsyncMock())

    await service._fragmentos(Importer(None, None, None), {}, {})

    assert lotes == [["HOOKS"], ["MEMORY"]], "a memória tem que ir num lote separado"
    assert any("memória" in a.lower() or "memoria" in a.lower() for a in service._estado["avisos"])
    service._config.assert_awaited()  # o resto da integração seguiu


def test_transcrito_ilegivel_vira_aviso_e_nao_sumico(tmp_path, monkeypatch):
    """Não dar para ler é falha, não é o caso normal da memória órfã: os dois acabam fora da
    importação, mas só um deles a pessoa pode resolver, e por isso só um vira aviso."""
    from app import codex_integracao
    origem = tmp_path / "projects"
    (origem / "-repo" / "memory").mkdir(parents=True)
    (origem / "-repo" / "memory" / "MEMORY.md").write_text("indice")
    (origem / "-repo" / "sessao.jsonl").write_text('{"type":"user"}\n')

    def abrir(self, *a, **kw):
        raise PermissionError("sem acesso")

    monkeypatch.setattr(codex_integracao.Path, "open", abrir)
    assert codex_integracao.copiar_memorias(origem, tmp_path / "stage") == ["-repo"]


def test_memoria_ilegivel_nao_derruba_os_outros_projetos(tmp_path, monkeypatch):
    from app import codex_integracao
    origem = tmp_path / "projects"
    for nome in ("-a-quebrado", "-b-bom"):
        (origem / nome / "memory").mkdir(parents=True)
        (origem / nome / "memory" / "MEMORY.md").write_text(nome)
        (origem / nome / "sessao.jsonl").write_text('{"type":"user"}\n')

    original = codex_integracao.shutil.copytree

    def copytree(src, dst, **kwargs):
        if "-a-quebrado" in str(src):
            raise PermissionError("sem acesso")
        return original(src, dst, **kwargs)

    monkeypatch.setattr(codex_integracao.shutil, "copytree", copytree)
    erros = codex_integracao.copiar_memorias(origem, tmp_path / "stage")

    # Memória é opt-in aditivo: um projeto ilegível não pode derrubar a reconciliação inteira.
    assert erros == ["-a-quebrado"]
    assert (tmp_path / "stage" / "-b-bom" / "memory" / "MEMORY.md").read_text() == "-b-bom"


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


async def test_atualizar_e_aguardar_compartilha_a_reconciliacao(tmp_path, monkeypatch):
    service = IntegracaoCodex(tmp_path, tmp_path / ".codex")
    iniciou = asyncio.Event()
    liberar = asyncio.Event()
    chamadas = []

    async def rodada(motivo, forcar):
        chamadas.append((motivo, forcar))
        iniciou.set()
        await liberar.wait()
        service._estado = {**service.status(), "estado": "ok"}
        return service.status()

    monkeypatch.setattr(service, "reconciliar", rodada)
    primeira = asyncio.create_task(service.atualizar_e_aguardar(forcar=True))
    await iniciou.wait()
    segunda = asyncio.create_task(service.atualizar_e_aguardar(forcar=True))
    await asyncio.sleep(0)
    assert not primeira.done() and not segunda.done()
    liberar.set()

    assert (await primeira)["estado"] == "ok"
    assert (await segunda)["estado"] == "ok"
    assert chamadas == [("manual", True)]


async def test_pedido_manual_durante_rodada_automatica_forca_segunda_rodada(tmp_path, monkeypatch):
    service = IntegracaoCodex(tmp_path, tmp_path / ".codex")
    iniciou = asyncio.Event()
    liberar = asyncio.Event()
    chamadas = []

    async def rodada(motivo, forcar):
        chamadas.append((motivo, forcar))
        if not forcar:
            iniciou.set()
            await liberar.wait()
        service._estado = {**service.status(), "estado": "ok"}
        return service.status()

    monkeypatch.setattr(service, "reconciliar", rodada)
    await service.iniciar("sessao", False)
    await iniciou.wait()
    manual = asyncio.create_task(service.atualizar_e_aguardar(forcar=True))
    liberar.set()

    assert (await manual)["estado"] == "ok"
    assert chamadas == [("sessao", False), ("manual", True)]


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
async def test_servidor_removido_da_fonte_com_enabled_particular_sai_inteiro(tmp_path):
    # Medido: o servidor sai do Claude, o Codex fica com `enabled = false` (campo particular) e
    # sem transporte -> `config/batchWrite` recusa a tabela ("invalid transport") e a integracao
    # inteira parava em erro. Sem command/url nao ha servidor: a entrada tem que sumir.
    home = _home(tmp_path)
    (home / ".claude.json").write_text(json.dumps({"mcpServers": {"local-probe": {"command": "echo", "args": ["primeiro"]}}}))
    cfg = home / ".codex/config.toml"
    service = IntegracaoCodex(home, home / ".codex")
    assert (await service.reconciliar())["estado"] == "ok"
    cfg.write_text(cfg.read_text().replace('[mcp_servers.local-probe]\n', '[mcp_servers.local-probe]\nenabled = false\n'))
    assert tomllib.loads(cfg.read_text())["mcp_servers"]["local-probe"]["enabled"] is False
    (home / ".claude.json").write_text(json.dumps({"mcpServers": {}}))
    depois = await service.reconciliar()
    assert depois["estado"] in ("ok", "parcial"), depois
    assert "local-probe" not in tomllib.loads(cfg.read_text()).get("mcp_servers", {})
    # A entrada foi removida: um aviso de "campos particulares preservados" mentiria.
    assert not any(a.get("codigo") == "aviso_config_campos_particulares" for a in depois.get("avisos", [])), depois["avisos"]


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

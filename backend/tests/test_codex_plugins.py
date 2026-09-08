"""Reconciliação real de plugins locais, sem rede de marketplace nem autenticação."""

import json
import os
from pathlib import Path
import shutil
import tomllib

import pytest

from app.codex_importador import CodexNativo
from app.codex_integracao import IntegracaoCodex


pytestmark = [pytest.mark.integration, pytest.mark.skipif(
    os.environ.get("RUN_CODEX_INTEGRATION") != "1" or not shutil.which("codex"),
    reason="Exige RUN_CODEX_INTEGRATION=1 e Codex CLI; usa somente HOME temporária",
)]

GERENCIADO = "gerenciado@hangar-local"
EXCLUSIVO = "exclusivo@hangar-local"


def _json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")


def _plugin(mercado, nome, versao):
    root = mercado / "plugins" / nome
    _json(root / ".codex-plugin/plugin.json", {
        "name": nome, "version": versao, "description": "Plugin temporário",
        "hooks": "./hooks/hooks.json",
    })
    _json(root / "hooks/hooks.json", {"hooks": {"SessionEnd": [{"hooks": [
        {"type": "command", "command": "echo fim", "timeout": 600},
    ]}]}})
    return root


@pytest.fixture
def instalacao(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    (home / ".codex").mkdir()
    mercado = tmp_path / "marketplace"
    _json(mercado / ".claude-plugin/marketplace.json", {
        "name": "hangar-local", "owner": {"name": "Hangar"}, "plugins": [
            {"name": n, "source": f"./plugins/{n}"} for n in ("gerenciado", "exclusivo")
        ],
    })
    gerenciado = _plugin(mercado, "gerenciado", "1.0.0")
    _plugin(mercado, "exclusivo", "1.0.0")
    _json(home / ".claude/settings.json", {"enabledPlugins": {GERENCIADO: True}})
    _json(home / ".claude/plugins/known_marketplaces.json", {"hangar-local": {
        "source": {"source": "directory", "path": str(mercado)}, "installLocation": str(mercado),
    }})
    _json(home / ".claude/plugins/installed_plugins.json", {"version": 2, "plugins": {
        GERENCIADO: [{"scope": "user", "installPath": str(gerenciado), "version": "1.0.0"}],
    }})
    return IntegracaoCodex(home, home / ".codex"), mercado


def _registro(service):
    return json.loads((service.raiz / "estado.json").read_text(encoding="utf-8"))


def _config(service):
    return tomllib.loads((service.codex_home / "config.toml").read_text(encoding="utf-8"))


def _timeout_plugin(root):
    data = json.loads((root / "hooks/hooks.json").read_text(encoding="utf-8"))
    return data["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"]


async def _exclusivo(service, mercado):
    async with CodexNativo(service.home, service.codex_home) as codex:
        await codex.cli(["plugin", "marketplace", "add", str(mercado), "--json"])
        return Path((await codex.instalar_plugin(EXCLUSIVO))["installedPath"])


async def test_importa_registry_atualiza_versao_e_preserva_plugin_exclusivo(instalacao):
    service, mercado = instalacao
    exclusivo = await _exclusivo(service, mercado)
    antes = (exclusivo / "hooks/hooks.json").read_bytes()

    primeira = await service.reconciliar()
    assert primeira["estado"] == "ok", primeira
    registro = _registro(service)["plugins"]
    assert set(registro) == {GERENCIADO}
    assert registro[GERENCIADO]["versao"] == "1.0.0"
    assert _timeout_plugin(Path(registro[GERENCIADO]["path"])) == 3
    assert (exclusivo / "hooks/hooks.json").read_bytes() == antes

    _plugin(mercado, "gerenciado", "2.0.0")
    segunda = await service.reconciliar(forcar=True)
    assert segunda["estado"] == "ok", segunda
    registro = _registro(service)["plugins"]
    assert registro[GERENCIADO]["versao"] == "2.0.0"
    assert _timeout_plugin(Path(registro[GERENCIADO]["path"])) == 3
    assert _config(service)["plugins"][EXCLUSIVO]["enabled"] is True
    assert (exclusivo / "hooks/hooks.json").read_bytes() == antes


@pytest.mark.parametrize("remover", [False, True])
async def test_desabilitar_ou_remover_da_fonte_preserva_cache_e_exclusivo(instalacao, remover):
    service, mercado = instalacao
    exclusivo = await _exclusivo(service, mercado)
    assert (await service.reconciliar())["estado"] == "ok"
    cache = Path(_registro(service)["plugins"][GERENCIADO]["path"])
    antes = (cache / "hooks/hooks.json").read_bytes()
    _json(service.home / ".claude/settings.json", {"enabledPlugins": {} if remover else {GERENCIADO: False}})

    result = await service.reconciliar()
    assert result["estado"] == "ok", result
    assert _config(service)["plugins"][GERENCIADO]["enabled"] is False
    assert _config(service)["plugins"][EXCLUSIVO]["enabled"] is True
    assert GERENCIADO not in _registro(service)["plugins"]
    assert (cache / "hooks/hooks.json").read_bytes() == antes
    assert exclusivo.is_dir()


async def test_atualizacao_externa_adota_versao_viva_e_reaplica_timeout(instalacao):
    service, mercado = instalacao
    assert (await service.reconciliar())["estado"] == "ok"
    antigo = Path(_registro(service)["plugins"][GERENCIADO]["path"])
    antigo_hook = (antigo / "hooks/hooks.json").read_bytes()
    _plugin(mercado, "gerenciado", "2.0.0")
    async with CodexNativo(service.home, service.codex_home) as codex:
        novo = Path((await codex.instalar_plugin(GERENCIADO))["installedPath"])
    assert _timeout_plugin(novo) == 600
    # Simula uma versão antiga ainda retida no cache: existência não prova a versão ativa.
    (antigo / "hooks").mkdir(parents=True, exist_ok=True)
    (antigo / "hooks/hooks.json").write_bytes(antigo_hook)

    result = await service.reconciliar(forcar=False)
    assert result["estado"] == "ok", result
    registro = _registro(service)["plugins"][GERENCIADO]
    assert registro["versao"] == "2.0.0"
    assert Path(registro["path"]) == novo
    assert _timeout_plugin(novo) == 3


async def test_marketplace_homonimo_de_outra_origem_nao_e_adotado(instalacao, tmp_path):
    service, _ = instalacao
    outro = tmp_path / "outro-marketplace"
    _json(outro / ".claude-plugin/marketplace.json", {
        "name": "hangar-local", "owner": {"name": "Outro"}, "plugins": [
            {"name": "gerenciado", "source": "./plugins/gerenciado"},
        ],
    })
    _plugin(outro, "gerenciado", "9.0.0")
    async with CodexNativo(service.home, service.codex_home) as codex:
        await codex.cli(["plugin", "marketplace", "add", str(outro), "--json"])
        cache = Path((await codex.instalar_plugin(GERENCIADO))["installedPath"])
        await codex.request("config/batchWrite", {"edits": [{
            "keyPath": f'plugins."{GERENCIADO}".enabled', "value": False, "mergeStrategy": "replace",
        }]})
    antes = (cache / "hooks/hooks.json").read_bytes()

    result = await service.reconciliar(forcar=True)
    assert result["estado"] == "parcial", result
    assert any(error["codigo"] == "erro_marketplace_origem" for error in result["erros"])
    assert GERENCIADO not in _registro(service).get("plugins", {})
    assert _config(service)["plugins"][GERENCIADO]["enabled"] is False
    assert (cache / "hooks/hooks.json").read_bytes() == antes


@pytest.mark.parametrize("preinstalado", [False, True])
async def test_marketplace_com_nome_nativo_diferente_atualiza_e_desabilita_identidade_real(instalacao, preinstalado):
    service, mercado = instalacao
    nativo = "gerenciado@codex-local"
    _json(mercado / ".agents/plugins/marketplace.json", {
        "name": "codex-local", "owner": {"name": "Hangar"},
        "plugins": [{"name": "gerenciado", "source": "./plugins/gerenciado"}],
    })
    if preinstalado:
        async with CodexNativo(service.home, service.codex_home) as codex:
            await codex.cli(["plugin", "marketplace", "add", str(mercado), "--json"])
            await codex.instalar_plugin(nativo)

    primeira = await service.reconciliar()
    assert primeira["estado"] == "ok", primeira
    assert _registro(service)["plugins"][GERENCIADO]["id_codex"] == nativo
    assert GERENCIADO not in _config(service)["plugins"]
    assert _config(service)["plugins"][nativo]["enabled"] is True

    _plugin(mercado, "gerenciado", "2.0.0")
    segunda = await service.reconciliar(forcar=True)
    assert segunda["estado"] == "ok", segunda
    assert _registro(service)["plugins"][GERENCIADO]["versao"] == "2.0.0"
    assert not _registro(service)["plugins_pendentes"]
    _json(service.home / ".claude/settings.json", {"enabledPlugins": {GERENCIADO: False}})
    assert (await service.reconciliar())["estado"] == "ok"
    assert _config(service)["plugins"][nativo]["enabled"] is False

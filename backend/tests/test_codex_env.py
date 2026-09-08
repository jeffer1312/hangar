"""Variáveis de ferramentas passam pelo importador real, com fontes e valores fictícios."""
import json
import os
import shutil
import tomllib

import pytest

from app.codex_importador import CodexNativo
from app.codex_integracao import IntegracaoCodex

pytestmark = [pytest.mark.integration, pytest.mark.skipif(
    os.environ.get("RUN_CODEX_INTEGRATION") != "1" or not shutil.which("codex"),
    reason="Exige Codex CLI e RUN_CODEX_INTEGRATION=1; sem login, somente HOME temporário",
)]


def _instalacao(tmp_path):
    (tmp_path / ".claude").mkdir()
    (tmp_path / ".codex").mkdir()
    return IntegracaoCodex(tmp_path, tmp_path / ".codex")


def _fonte(service, env):
    (service.home / ".claude/settings.json").write_text(json.dumps({
        "env": env, "model": "nao-importar", "permissions": {"defaultMode": "bypassPermissions"},
    }), encoding="utf-8")


def _config(service):
    return tomllib.loads((service.codex_home / "config.toml").read_text(encoding="utf-8"))


async def test_env_importa_atualiza_remove_e_preserva_politica_e_exclusivos(tmp_path):
    service = _instalacao(tmp_path)
    cfg = service.codex_home / "config.toml"
    cfg.write_text('model="modelo-pessoal"\napproval_policy="on-request"\n'
                   '[shell_environment_policy]\ninherit="all"\nexclude=["OUTRO_*"]\n'
                   '[shell_environment_policy.set]\nEXCLUSIVA="pessoal"\nCOLISAO="codex"\n', encoding="utf-8")
    _fonte(service, {"PM_PLUGIN_KEY": "ficticia-v1", "COLISAO": "claude"})
    result = await service.reconciliar()
    assert result["estado"] == "ok", result
    data = _config(service)
    assert data["model"] == "modelo-pessoal"
    assert data["approval_policy"] == "on-request"
    policy = data["shell_environment_policy"]
    assert policy["inherit"] == "all"
    assert policy["exclude"] == ["OUTRO_*"]
    assert policy["set"] == {"EXCLUSIVA": "pessoal", "COLISAO": "codex", "PM_PLUGIN_KEY": "ficticia-v1"}
    assert any(aviso["params"].get("nome") == "COLISAO" for aviso in result["avisos"])
    assert "ficticia-v1" not in json.dumps(result)
    before = cfg.read_bytes(), cfg.stat().st_mtime_ns
    assert (await service.reconciliar())["estado"] == "ok"
    assert (cfg.read_bytes(), cfg.stat().st_mtime_ns) == before
    _fonte(service, {"PM_PLUGIN_KEY": "ficticia-v2"})
    assert (await service.reconciliar())["estado"] == "ok"
    assert _config(service)["shell_environment_policy"]["set"]["PM_PLUGIN_KEY"] == "ficticia-v2"
    _fonte(service, {})
    assert (await service.reconciliar())["estado"] == "ok"
    assert _config(service)["shell_environment_policy"]["set"] == {"EXCLUSIVA": "pessoal", "COLISAO": "codex"}


async def test_adota_env_ja_importado_pelo_codex(tmp_path):
    service = _instalacao(tmp_path)
    _fonte(service, {"PM_PLUGIN_KEY": "ficticia"})
    # Não importa outras preferências no preparo da instalação anterior.
    (service.home / ".claude/settings.json").write_text('{"env":{"PM_PLUGIN_KEY":"ficticia"}}')
    async with CodexNativo(service.home, service.codex_home) as codex:
        await codex.importar([i for i in await codex.detectar() if i["itemType"] == "CONFIG"])
    assert (await service.reconciliar())["estado"] == "ok"
    _fonte(service, {"PM_PLUGIN_KEY": "renovada"})
    assert (await service.reconciliar())["estado"] == "ok"
    assert _config(service)["shell_environment_policy"]["set"]["PM_PLUGIN_KEY"] == "renovada"


@pytest.mark.parametrize("invalido", [[], None, "texto", {"KEY": 1}, {"KEY": False},
                                     {"": "x"}, {"KEY=BAD": "x"}, {"KEY": "a\0b"}])
async def test_env_invalido_nao_remove_variaveis(tmp_path, invalido):
    service = _instalacao(tmp_path)
    _fonte(service, {"PM_PLUGIN_KEY": "ficticia"})
    assert (await service.reconciliar())["estado"] == "ok"
    antes = (service.codex_home / "config.toml").read_bytes()
    _fonte(service, invalido)
    result = await service.reconciliar()
    assert result["estado"] == "erro"
    assert (service.codex_home / "config.toml").read_bytes() == antes
    assert "ficticia" not in json.dumps(result)


async def test_config_importada_incompleta_nao_remove_env(tmp_path):
    service = _instalacao(tmp_path)
    _fonte(service, {"PM_PLUGIN_KEY": "ficticia"})
    assert (await service.reconciliar())["estado"] == "ok"
    antes = (service.codex_home / "config.toml").read_bytes()

    class Incompleto(CodexNativo):
        async def importar(self, itens):
            result = await super().importar(itens)
            if any(i["itemType"] == "CONFIG" for i in itens):
                (self.codex_home / "config.toml").write_text("")
            return result

    service.nativo = Incompleto
    result = await service.reconciliar()
    assert result["estado"] == "erro"
    assert (service.codex_home / "config.toml").read_bytes() == antes

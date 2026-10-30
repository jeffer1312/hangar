"""Configuração oficial numa HOME temporária; nenhum pedido de inferência."""
import json
import shutil
import tomllib

import pytest
from fastapi import HTTPException

from app import codex_opcoes, harness_api, runtime_config
from app.codex_integracao import IntegracaoCodex
from app.codex_opcoes import ler_opcoes, salvar_opcoes


@pytest.fixture(autouse=True)
def _runtime_config_isolado(tmp_path, monkeypatch):
    monkeypatch.setattr(runtime_config, "_backend_config_base", lambda: str(tmp_path / "hangar-config"))


@pytest.mark.skipif(not shutil.which("codex"), reason="Codex CLI necessário para validar o escritor TOML")
@pytest.mark.parametrize("anterior", [None, 300000])
async def test_contexto_nativo_preserva_e_restaura_config(tmp_path, anterior):
    codex_home = tmp_path / ".codex"
    codex_home.mkdir()
    path = codex_home / "config.toml"
    path.write_text('# comentário pessoal\nmodel = "gpt-6-astra"\nmodel_auto_compact_token_limit = 250000\n'
                    + (f"model_context_window = {anterior}\n" if anterior else ""))
    service = IntegracaoCodex(home=tmp_path, codex_home=codex_home)
    # Sem valor no config.toml vale o 1M do Hangar; um valor escrito à mão é respeitado.
    assert ler_opcoes(service)["contexto_estendido"] is (anterior is None)
    assert (await salvar_opcoes(service, True))["contexto_estendido"]
    assert (await salvar_opcoes(service, True))["contexto_estendido"]
    config = tomllib.loads(path.read_text())
    assert config["model_context_window"] == 1000000
    assert config["model_auto_compact_token_limit"] == 900000
    await salvar_opcoes(service, False)
    config = tomllib.loads(path.read_text())
    assert config.get("model_context_window") == anterior
    assert config["model_auto_compact_token_limit"] == 250000
    assert config["model"] == "gpt-6-astra"
    assert "# comentário pessoal" in path.read_text()


@pytest.mark.skipif(not shutil.which("codex"), reason="Codex CLI necessário para validar o escritor TOML")
@pytest.mark.parametrize("compactacao", [900000, 250000])
async def test_contexto_preexistente_desliga_sem_restaurar_um_milhao(tmp_path, compactacao):
    path = tmp_path / "config.toml"
    path.write_text(f"model_context_window = 1000000\nmodel_auto_compact_token_limit = {compactacao}\n")
    service = IntegracaoCodex(home=tmp_path, codex_home=tmp_path)
    if compactacao != 900000:
        await salvar_opcoes(service, True)
        assert tomllib.loads(path.read_text())["model_auto_compact_token_limit"] == 900000
    await salvar_opcoes(service, False)
    config = tomllib.loads(path.read_text())
    assert "model_context_window" not in config
    assert config.get("model_auto_compact_token_limit") == (None if compactacao == 900000 else compactacao)


async def test_sem_configuracao_o_padrao_e_um_milhao_e_desligar_fica_gravado(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text('model = "gpt-6.1-sol"\n')
    service = IntegracaoCodex(home=tmp_path, codex_home=tmp_path)
    assert ler_opcoes(service)["contexto_estendido"] is True
    assert (await salvar_opcoes(service, False))["contexto_estendido"] is False
    assert runtime_config.get("codex_extended_context") is False
    assert ler_opcoes(service)["contexto_estendido"] is False
    assert path.read_text() == 'model = "gpt-6.1-sol"\n'


def test_valor_explicito_no_config_vence_o_desligado(tmp_path):
    (tmp_path / "config.toml").write_text("model_context_window = 1000000\n")
    runtime_config.aplicar({"codex_extended_context": False})
    assert ler_opcoes(IntegracaoCodex(home=tmp_path, codex_home=tmp_path))["contexto_estendido"] is True


@pytest.mark.skipif(not shutil.which("codex"), reason="Codex CLI necessário para validar o escritor TOML")
async def test_religar_limpa_o_desligado(tmp_path):
    runtime_config.aplicar({"codex_extended_context": False})
    service = IntegracaoCodex(home=tmp_path, codex_home=tmp_path)
    assert (await salvar_opcoes(service, True))["contexto_estendido"] is True
    assert runtime_config.get("codex_extended_context") is True
    assert tomllib.loads((tmp_path / "config.toml").read_text())["model_context_window"] == 1000000


def test_catalogo_anuncia_teto_real_sem_inventar_um_milhao(tmp_path):
    service = IntegracaoCodex(home=tmp_path, codex_home=tmp_path)
    (tmp_path / "models_cache.json").write_text(json.dumps({"models": [
        {"slug": "gpt-6-astra", "context_window": 272000, "max_context_window": 872000},
        {"slug": "pequeno", "context_window": 128000, "max_context_window": 128000},
        {"slug": "interno", "context_window": 272000, "max_context_window": 872000, "visibility": "hide"},
    ]}))
    assert ler_opcoes(service)["modelos"] == [{"model": "gpt-6-astra", "default": 272000, "max": 872000}]


async def test_interruptor_antigo_da_voz_e_aceito_e_ignorado(monkeypatch):
    def nao_gravar(*_, **__):
        raise AssertionError("a voz não tem mais interruptor no runtime-config")
    monkeypatch.setattr(runtime_config, "aplicar", nao_gravar)
    pedidos = []
    async def salvar(_servico, contexto):
        pedidos.append(contexto)
        return {"contexto_estendido": contexto}
    monkeypatch.setattr(codex_opcoes, "salvar_opcoes", salvar)
    resposta = await harness_api.codex_opcoes_salvar(harness_api.CodexOpcoesBody(
        contexto_estendido=True, codex_voice_beta=True))
    assert resposta == {"contexto_estendido": True, "codex_voice_beta": True}
    assert pedidos == [True]


async def test_falha_do_contexto_vira_409(monkeypatch):
    async def falhar(*_):
        raise RuntimeError("config Codex indisponível")
    monkeypatch.setattr(codex_opcoes, "salvar_opcoes", falhar)
    with pytest.raises(HTTPException) as exc:
        await harness_api.codex_opcoes_salvar(harness_api.CodexOpcoesBody(contexto_estendido=True))
    assert exc.value.status_code == 409

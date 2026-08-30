"""Liga/desliga do resumo do pensamento — a chave mora no settings.json do Claude Code.

O que esta suite trava: preservar o resto do arquivo (o interruptor nao pode custar os hooks e as
permissoes de quem usa), nao mexer no que nao da pra ler, e o campo chegar pela mesma tela de
config (runtime_config) sem ser gravado no runtime-config.json.
"""
import json

import pytest

from app import pensamento, runtime_config as rc


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(pensamento, "compartilhado", lambda: tmp_path)
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path)
    yield


def _settings(tmp_path) -> dict:
    return json.loads((tmp_path / "settings.json").read_text(encoding="utf-8"))


def test_sem_arquivo_e_desligado(tmp_path):
    assert pensamento.ler() is False
    assert pensamento.definido() is False


def test_gravar_preserva_o_resto_do_arquivo(tmp_path):
    # O que mais dói se quebrar: o settings.json do usuario tem hooks, permissions e statusLine.
    # Gravar um dict parcial apagaria a configuracao inteira por causa de um interruptor.
    (tmp_path / "settings.json").write_text(json.dumps({
        "model": "opus[1m]", "hooks": {"Stop": [{"hooks": []}]}, "outputStyle": "Concise",
    }), encoding="utf-8")

    pensamento.gravar(True)

    d = _settings(tmp_path)
    assert d["showThinkingSummaries"] is True
    assert d["model"] == "opus[1m]"
    assert d["hooks"] == {"Stop": [{"hooks": []}]}
    assert d["outputStyle"] == "Concise"
    assert pensamento.ler() is True


def test_desligar_deixa_a_chave_como_false(tmp_path):
    # false EXPLICITO, nao a chave removida: ausencia e "nunca escolhi", e a tela precisa saber a
    # diferenca pra marcar a linha como editada.
    pensamento.gravar(True)
    pensamento.gravar(False)
    assert _settings(tmp_path)["showThinkingSummaries"] is False
    assert pensamento.ler() is False
    assert pensamento.definido() is True


def test_arquivo_ilegivel_nao_e_sobrescrito(tmp_path):
    # JSON quebrado (editado a mao, escrita cortada): reescrever daqui apagaria tudo. Recusa alto —
    # a tela mostra o erro em vez de dizer que salvou.
    alvo = tmp_path / "settings.json"
    alvo.write_text('{"model": "opus"', encoding="utf-8")
    with pytest.raises(RuntimeError):
        pensamento.gravar(True)
    assert alvo.read_text(encoding="utf-8") == '{"model": "opus"'
    assert pensamento.ler() is False


def test_chega_pela_tela_de_config_sem_ir_pro_runtime_config(tmp_path):
    rc.aplicar({"mostrar_pensamento": True})
    assert pensamento.ler() is True
    assert rc.estado()["mostrar_pensamento"]["valor"] is True
    assert rc.estado()["mostrar_pensamento"]["origem"] == "app"
    # A verdade e o settings.json: uma copia no runtime-config.json divergiria no minuto em que
    # alguem editasse o arquivo do Claude a mao.
    assert "mostrar_pensamento" not in json.loads(
        (tmp_path / "runtime-config.json").read_text(encoding="utf-8"))


def test_tipo_errado_e_recusado(tmp_path):
    with pytest.raises(ValueError):
        rc.aplicar({"mostrar_pensamento": "sim"})
    assert pensamento.definido() is False


def test_campo_invalido_no_mesmo_salvar_nao_grava_o_interruptor(tmp_path):
    # A tela manda o rascunho INTEIRO num POST so. Se outro campo do mesmo Salvar for invalido, a
    # pessoa ve o erro e o rascunho fica intacto — entao o settings.json NAO pode ter sido mexido,
    # senao ela acha que nao ligou o resumo e a proxima sessao nasce com ele ligado.
    with pytest.raises(ValueError):
        rc.aplicar({"mostrar_pensamento": True, "upload_retention_days": "nao-e-numero"})
    assert pensamento.definido() is False
    assert pensamento.ler() is False

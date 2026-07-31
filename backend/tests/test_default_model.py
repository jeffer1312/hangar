"""O undo do efeito colateral global do `/model <id>`.

Contexto medido (31/07/2026, claude 2.1.220): numa sessao de motor, `/model kimi-for-coding`
responde "Set model to kimi-for-coding and saved as your default for new sessions" — grava o id no
settings.json GLOBAL. Sem o restore, uma sessao nova da conta Anthropic nasceria pedindo um modelo
que a API da Anthropic nao conhece.
"""
import json

from app import default_model


def _settings(tmp_path, conteudo: dict | str):
    p = tmp_path / "settings.json"
    p.write_text(conteudo if isinstance(conteudo, str) else json.dumps(conteudo), encoding="utf-8")
    return p


def test_restore_devolve_o_valor_anterior(tmp_path):
    _settings(tmp_path, {"model": "claude-opus-5", "permissions": {"allow": ["Bash"]}})
    antes = default_model.snapshot(tmp_path)

    # simula o que o Claude Code faz ao processar `/model kimi-for-coding`
    _settings(tmp_path, {"model": "kimi-for-coding", "permissions": {"allow": ["Bash"]}})

    assert default_model.restore(tmp_path, antes) is True
    d = json.loads((tmp_path / "settings.json").read_text())
    assert d["model"] == "claude-opus-5"
    assert d["permissions"] == {"allow": ["Bash"]}  # o resto do arquivo fica intacto


def test_restore_remove_a_chave_que_nao_existia(tmp_path):
    _settings(tmp_path, {"statusLine": {"type": "command"}})
    antes = default_model.snapshot(tmp_path)
    _settings(tmp_path, {"statusLine": {"type": "command"}, "model": "k3-256k"})

    assert default_model.restore(tmp_path, antes) is True
    d = json.loads((tmp_path / "settings.json").read_text())
    assert "model" not in d
    assert d["statusLine"] == {"type": "command"}


def test_restore_e_noop_quando_nada_mudou(tmp_path):
    _settings(tmp_path, {"model": "claude-opus-5"})
    antes = default_model.snapshot(tmp_path)
    assert default_model.restore(tmp_path, antes) is False


def test_arquivo_quebrado_nao_e_tocado(tmp_path):
    # JSON invalido editado a mao: perder a config do usuario e pior que deixar o default trocado.
    p = _settings(tmp_path, '{"model": "x",,,}')
    antes = default_model.snapshot(tmp_path)
    assert antes is None
    assert default_model.restore(tmp_path, antes) is False
    assert p.read_text() == '{"model": "x",,,}'


def test_arquivo_ausente_vira_ausencia_da_chave(tmp_path):
    antes = default_model.snapshot(tmp_path)   # sem settings.json nenhum
    _settings(tmp_path, {"model": "k3"})
    assert default_model.restore(tmp_path, antes) is True
    assert "model" not in json.loads((tmp_path / "settings.json").read_text())


def test_restore_quando_aterrissar_espera_a_escrita_do_claude_code(tmp_path, monkeypatch):
    # Medido: o settings.json so muda ~0.8s DEPOIS do Enter — repor antes disso e um no-op e o id
    # do motor aterrissa em seguida, vazado e calado.
    _settings(tmp_path, {"model": "claude-opus-5"})
    antes = default_model.snapshot(tmp_path)

    leituras = {"n": 0}

    def sleep_que_simula_o_claude_code(_s):
        leituras["n"] += 1
        if leituras["n"] == 3:            # a escrita atrasada aterrissa no 3o tick
            _settings(tmp_path, {"model": "kimi-for-coding"})

    monkeypatch.setattr(default_model.time, "sleep", sleep_que_simula_o_claude_code)
    assert default_model.restore_quando_aterrissar(tmp_path, antes) is True
    assert json.loads((tmp_path / "settings.json").read_text())["model"] == "claude-opus-5"


def test_restore_quando_aterrissar_pega_a_escrita_atrasada_apos_repor(tmp_path, monkeypatch):
    # Segunda conferencia: a escrita chega DEPOIS de repormos. Sem ela, o arquivo termina com o id
    # do motor mesmo tendo passado pelo restore.
    _settings(tmp_path, {"model": "claude-opus-5"})
    antes = default_model.snapshot(tmp_path)
    _settings(tmp_path, {"model": "k3"})      # 1a escrita: ja mudou, a sonda sai de cara

    def sleep_que_reescreve(_s):
        _settings(tmp_path, {"model": "k3"})  # o CC grava de novo logo depois do nosso restore

    monkeypatch.setattr(default_model.time, "sleep", sleep_que_reescreve)
    assert default_model.restore_quando_aterrissar(tmp_path, antes) is True
    assert json.loads((tmp_path / "settings.json").read_text())["model"] == "claude-opus-5"

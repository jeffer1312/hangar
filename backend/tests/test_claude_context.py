"""Contexto da sessão Claude lido do transcript, para quem não usa a statusline do Hangar."""
import json

from app import claude_context as cc


def _resposta(model="claude-opus-5-5", entrada=2, lido=0, escrito=0, lateral=False, tipo="assistant"):
    return {"type": tipo, "isSidechain": lateral, "message": {"model": model, "usage": {
        "input_tokens": entrada, "cache_read_input_tokens": lido, "cache_creation_input_tokens": escrito,
        "output_tokens": 50}}}


def _transcript(tmp_path, *linhas):
    p = tmp_path / "s.jsonl"
    p.write_text("".join(json.dumps(l) + "\n" for l in linhas) + '{"truncad', encoding="utf-8")
    return p


def test_usa_a_ultima_resposta_do_agente_principal(tmp_path):
    p = _transcript(tmp_path,
                    _resposta(lido=10_000),
                    _resposta(entrada=3, lido=90_000, escrito=2_000),
                    # Subagente e resposta sintética não são o contexto da conversa.
                    _resposta(lido=5, lateral=True),
                    _resposta(model="<synthetic>", entrada=0))
    assert cc.from_transcript(p, tmp_path) == {"used": 92_003, "window": 200_000}


def test_janela_de_1m_pelo_modelo_configurado_ou_pelo_uso(tmp_path):
    p = _transcript(tmp_path, _resposta(lido=90_000))
    (tmp_path / "settings.json").write_text(json.dumps({"model": "opus[1m]"}), encoding="utf-8")
    assert cc.from_transcript(p, tmp_path)["window"] == 1_000_000
    # Uso acima da janela padrão só cabe na de 1M, mesmo sem a configuração dizer.
    assert cc.window(250_000, tmp_path / "sem-config") == 1_000_000
    assert cc.window(150_000, tmp_path / "sem-config") == 200_000


def test_sem_resposta_ou_sem_arquivo_e_none(tmp_path):
    assert cc.from_transcript(_transcript(tmp_path, {"type": "user", "message": {"content": "oi"}}), tmp_path) is None
    assert cc.from_transcript(tmp_path / "nao-existe.jsonl", tmp_path) is None
    assert cc.from_transcript(None) is None

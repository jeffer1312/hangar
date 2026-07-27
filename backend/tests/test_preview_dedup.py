"""Preview duplicado/piscando: o pane mostrava a msg JÁ commitada + a linha de status do TUI,
a supressão não pegava, e a bolha aparecia/sumia/voltava.

Cobre as duas metades do conserto:
  - extract_assistant_text corta a linha de status ("Making N scratchpad edit…")
  - preview_is_committed suprime também quando o commitado é PREFIXO do preview (o caso que
    escapava quando o verbo de status é desconhecido)
"""
from pathlib import Path

from app.preview import extract_assistant_text, _norm
from app.sse import preview_is_committed


PROSE = "Falta um dado decisivo: por que meu cliente nao recebeu turn/completed."


def test_extract_corta_linha_de_status_making():
    # "Making …" é o verbo novo do Claude Code que não estava no _TOOL_VERBS: sem o corte ele
    # grudava no fim da prosa e o preview virava prosa+chrome.
    pane = "\n".join([
        f"● {PROSE}",
        "  Making 1 scratchpad edit +130, running 2 shell commands · 1m 5s...",
    ])
    assert extract_assistant_text(pane) == PROSE


def test_suprime_quando_preview_e_o_bloco_ja_commitado():
    # caso 1 (já cobria): o pane ainda mostra o bloco que caiu no .jsonl
    assert preview_is_committed(PROSE, _norm(PROSE))


def test_suprime_quando_commitado_e_prefixo_do_preview():
    # caso 2 (o bug): chrome não cortado gruda no fim -> preview ⊃ commitado.
    preview = f"{PROSE}\n\nBlargling 3 widgets · 2m 1s..."   # verbo que ninguém previu
    assert preview_is_committed(preview, _norm(PROSE))


def test_nao_suprime_bloco_novo_de_verdade():
    # texto em voo diferente do commitado NÃO pode ser engolido, senão o preview morre.
    assert not preview_is_committed("Agora vou explicar outra coisa bem diferente.", _norm(PROSE))


def test_nao_suprime_trecho_curto():
    # piso de 16 chars: fragmento curto casaria por acidente.
    assert not preview_is_committed("ok", _norm(PROSE))


# --- Pi: o chrome que fecha o bloco em voo e a CAIXA do composer, nao a regua do Claude ---------
# pane_pi_working.txt e um `tmux capture-pane -p` REAL, tirado DURANTE um turno (Pi 0.82.1 +
# kimi-for-coding): o `● Tem sim, algumas formas:` esta a meio caminho de ser escrito. Sem a parada
# na caixa, o preview do celular vinha com a borda `╭───╮`/`╰───╯` e a statusline (🤖 modelo …)
# coladas no fim da prosa a CADA frame.

def _pane_pi_working() -> str:
    return (Path(__file__).parent / "fixtures" / "pane_pi_working.txt").read_text(encoding="utf-8")


def test_preview_pi_para_na_caixa_do_composer():
    txt = extract_assistant_text(_pane_pi_working(), "pi")
    assert txt.startswith("Tem sim, algumas formas:")
    assert txt.rstrip().endswith("Copiar uma mensagem antiga específica:")
    assert "╭" not in txt and "╰" not in txt      # borda da caixa fora
    assert "kimi-for-coding" not in txt           # statusline fora


def test_preview_pi_sem_provider_ainda_traz_o_chrome():
    # Trava o motivo do parametro existir: o MESMO pane lido como "claude" (o default) devolve o
    # texto com a caixa e a statusline grudadas — que era o bug.
    txt = extract_assistant_text(_pane_pi_working())
    assert "╭" in txt and "kimi-for-coding" in txt


def test_preview_claude_byte_identico_em_todos_os_panes():
    # Nao-regressao: o provider novo NAO pode mover um byte do que o Claude/Codex ja extraiam.
    # Compara o default contra a lista VAZIA de paradas extras em todo fixture de pane do repo.
    fx = Path(__file__).parent / "fixtures"
    for f in sorted(fx.glob("pane_*.txt")):
        pane = f.read_text(encoding="utf-8")
        assert extract_assistant_text(pane) == extract_assistant_text(pane, "provider-inexistente"), f.name

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


def test_status_de_ferramenta_mcp_nao_entra_no_preview():
    """"Calling <servidor>…" é chrome de tool MCP, não prosa do assistente.

    Sem ele na lista de verbos, a linha entrava no bloco em voo e — como aparece e some a cada
    chamada — o preview crescia e encolhia sozinho: o "pulo" que o usuário via na tela.
    """
    from app.preview import extract_assistant_text

    pane = "\n".join([
        "● Resposta do assistente em voo,",
        "  segunda linha da mesma prosa.",
        "Calling chrome-devtools…",
        "✻ Pondering… (22s · ↓ 678 tokens)",
    ])
    out = extract_assistant_text(pane)
    assert "segunda linha" in out
    assert "Calling" not in out
    assert "Pondering" not in out


def test_prosa_comecando_com_calling_nao_e_confundida_com_tool():
    """"Calling" solto é início de frase comum — não pode descartar o bloco.

    A lista de verbos decide TAMBÉM qual ● é tool-call: um falso positivo ali faz a função não achar
    bloco nenhum e devolver "" — preview vazio, que é pior que preview sujo.
    """
    from app.preview import extract_assistant_text

    pane = "\n".join([
        "● Calling this an edge case would be generous:",
        "  o arquivo não tem gráfico nem pivô.",
        "✻ Pondering… (3s)",
    ])
    out = extract_assistant_text(pane)
    assert "edge case" in out
    assert "não tem gráfico" in out


PANE_PI_TOOL_DEPOIS_DA_PROSA = """● Fechei o wire e subi o commit; o back valida na sequência.

● Bash cd "/home/jefferson/x" && dotnet run
 └ pid 326487
   t+35s: vivo

✻ Unfurling… (7m 11s)
"""


def test_preview_pi_pula_bloco_de_tool_sem_parenteses():
    # O Pi escreve "● Bash cd ..." SEM parenteses -> _TOOL_BLOCK_RE nao pega. Quem denuncia e a
    # linha de resultado (`└`) logo abaixo. Sem isto o comando virava a previa e o bloco em voo
    # trocava de altura a cada ferramenta (a conversa pulava no celular).
    out = extract_assistant_text(PANE_PI_TOOL_DEPOIS_DA_PROSA, "pi")
    assert out == "Fechei o wire e subi o commit; o back valida na sequência."


def test_prosa_pi_que_termina_em_arvore_continua_sendo_prosa():
    # Guarda contra o falso positivo: `└` DENTRO do proprio bloco (arvore desenhada na prosa) nao
    # pode descartar o bloco -- previa vazia e pior que previa suja.
    pane = "● Estrutura final:\n └ src/\n   └ wire.ts\n\n✻ Unfurling… (1s)\n"
    assert extract_assistant_text(pane, "pi").startswith("Estrutura final:")


def test_preview_claude_ignora_a_regra_do_pi():
    # `└` na linha seguinte a um ● do Claude e prosa desenhando arvore: o marcador dele e `⎿`.
    pane = "● Ficou assim:\n └ dist/\n\n✻ Thinking… (2s)\n"
    assert extract_assistant_text(pane).startswith("Ficou assim:")


def test_preview_pi_pula_grupo_de_bash_com_dois_pontos():
    # Terceira forma medida no pane real (01/08): "● Bash: 2 done • ctrl+o to toggle" com os filhos
    # em `├`. Dois-pontos no lugar do espaco, `├` no lugar do `└` -- mesma coisa, mesmo tratamento.
    pane = (
        "● Segue o resumo do que mudou no wire.\n"
        "\n"
        "● Bash: 2 done • ctrl+o to toggle\n"
        " ├ ● grep -rn \"Adicionar\" src/\n"
        " └ ● dotnet build\n"
        "\n"
        "✻ Unfurling… (2m)\n"
    )
    assert extract_assistant_text(pane, "pi") == "Segue o resumo do que mudou no wire."


def test_preview_pi_pula_as_quatro_formas_de_cabecalho_de_tool():
    # As quatro medidas no pane real (01/08/2026). Nenhuma casa com _TOOL_BLOCK_RE ("Nome(" colado);
    # todas tem filho em box-drawing na linha de baixo -- e e so isso que a regra olha.
    for cabecalho in (
        'Bash cd "/home/jefferson/x" && dotnet run',
        "Bash: 2 done • ctrl+o to toggle",
        "Write  (81 lines)",
        "Multiple Tools: 3 done • bash, chrome_devtools_navigate_page",
    ):
        pane = (
            "● A prosa que deve sobreviver.\n"
            "\n"
            f"● {cabecalho}\n"
            " └ pid 326487\n"
            "\n"
            "✻ Unfurling… (2m)\n"
        )
        assert extract_assistant_text(pane, "pi") == "A prosa que deve sobreviver.", cabecalho


def test_painel_de_tarefas_do_pi_continua_chegando_na_previa():
    # Mesma FORMA de uma ferramenta (cabecalho + filhos em box-drawing), mas e o unico bloco desses
    # que o usuario quer ver -- a bolha o mostra dobrado. Sem a excecao, a regra estrutural o comia.
    pane = (
        "● Todos (11/13)\n"
        "├─ ✓ Avisar back: revisar DDL\n"
        "└─ +3 more (3 completed)\n"
        "\n"
        "✻ Unfurling… (2m)\n"
    )
    assert extract_assistant_text(pane, "pi").startswith("Todos (11/13)")


def test_prosa_pi_que_apresenta_trecho_sem_dois_pontos_sobrevive():
    # Achado da review (01/08): a versao anterior olhava so a ESTRUTURA e protegia a prosa com um
    # guard de ":" no fim do cabecalho. Mas uma frase nao precisa terminar em dois-pontos pra
    # introduzir um trecho -- e ai o bloco era descartado e a previa ficava VAZIA.
    pane = (
        "● Aqui vai o trecho pra comparar\n"
        "│ codigo original\n"
        "│ codigo novo\n"
        "\n"
        "✻ Unfurling… (2s)\n"
    )
    out = extract_assistant_text(pane, "pi")
    assert out, "previa VAZIA: a prosa foi descartada como se fosse ferramenta"
    assert out.startswith("Aqui vai o trecho pra comparar")
    assert "codigo original" in out   # o trecho e PARTE do bloco de prosa, nao chrome pra cortar


def test_preview_pi_pula_edit_com_diff():
    # "● Edit  (2 edits)" desenha DIFF (│/▌), nao arvore -- forma medida no pane real.
    pane = (
        "● A prosa que deve sobreviver.\n"
        "\n"
        "● Edit  (2 edits)\n"
        " │ ▌57- const [printOpen, setPrintOpen] = React.useState(false)\n"
        "\n"
        "✻ Inspecting… (6m)\n"
    )
    assert extract_assistant_text(pane, "pi") == "A prosa que deve sobreviver."


PANE_COM_PAINEL_DE_SUBAGENTES = """● Dinheiro: a cobranca e por caractere do texto que voce manda.

────────────────────────────────────────────────────────────
❯ Sim, depois faz o streaming
────────────────────────────────────────────────────────────
  🤖 Opus5·1M (high✦) │ 📁 claude-cockpit [main*] │ 💵 $10.51
  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← 2 agents

  ● main
  ◯ general-purpose  Grepping textoFalavel usage sitewide  1m 34s · ↓ 105.1k tokens
"""


def test_painel_de_subagentes_nao_vira_previa():
    # O painel de subagentes marca o agente principal com o MESMO ● do bloco do assistente, e fica
    # no RODAPE -- como a varredura pega o ultimo ●, ele ganhava sempre e a previa virava
    # "main / ◯ general-purpose ...". O corte na ultima regua resolve por POSICAO.
    out = extract_assistant_text(PANE_COM_PAINEL_DE_SUBAGENTES)
    assert out == "Dinheiro: a cobranca e por caractere do texto que voce manda."


def test_pane_sem_regua_ainda_varre_tudo():
    # Fallback: sem regua (pane recem-aberto), o comportamento antigo vale -- previa e melhor que
    # nada, e nao ha rodape pra confundir.
    pane = "● Texto solto sem chrome nenhum\n"
    assert extract_assistant_text(pane) == "Texto solto sem chrome nenhum"


def test_overlay_do_model_tambem_corta_o_painel_de_subagentes():
    # Achado da review: o separador do overlay do /model e `▔` (U+2594), nao a regua reta -- nas
    # fixtures pane_model_picker_*.txt o _RULE_RE nao casa NADA. Sem reconhecer esse desenho, o
    # corte caia no fallback "varre tudo" e o painel de subagentes voltava a vencer.
    fx = Path(__file__).parent / "fixtures"
    pane = (fx / "pane_model_picker_opus.txt").read_text(encoding="utf-8") + (
        "\n  ● main\n"
        "  ◯ general-purpose  Grepping textoFalavel usage sitewide  1m 34s\n"
    )
    out = extract_assistant_text(pane)
    assert not out.startswith("main"), f"painel de subagentes vazou: {out[:60]!r}"


def test_caixa_do_composer_do_pi_tambem_corta():
    # O Pi desenha o composer com caixa arredondada e NUNCA regua reta -- sem incluir esse desenho,
    # o corte era no-op no caminho dele.
    pane = (
        "● Prosa do Pi em voo.\n"
        "╭──────────────────────────────╮\n"
        "│ digitar aqui                 │\n"
        "╰──────────────────────────────╯\n"
        "  🤖 k3 (high)\n"
        "\n"
        "  ● main\n"
        "  ◯ general-purpose  Grepping  1m 34s\n"
    )
    assert extract_assistant_text(pane, "pi") == "Prosa do Pi em voo."

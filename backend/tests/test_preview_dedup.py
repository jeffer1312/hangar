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


def test_painel_de_tarefas_nao_vira_previa():
    # DECISAO REVERTIDA em 03/08/2026 (antes o painel era o unico bloco desses que aparecia,
    # dobrado): o usuario NAO quer o "Todos (n/n)" na previa. Ele nao e prosa em voo -- e o mesmo
    # TodoWrite que vira ToolCard quando o turno fecha.
    pane = (
        "● Todos (11/13)\n"
        "├─ ✓ Avisar back: revisar DDL\n"
        "└─ +3 more (3 completed)\n"
        "\n"
        "✻ Unfurling… (2m)\n"
    )
    assert extract_assistant_text(pane, "pi") == ""


# Pane REAL (Pi 0.82.1 + k3, 03/08/2026) do frame que vazava: o spinner esta no quadro ASCII `*`
# (U+002A) -- fora de SPINNER_GLYPHS -- e por isso nao parava a varredura, que seguia engolindo a
# linha de status E o painel de Todos inteiro. Um frame em seis do ciclo `✻✽✶✺✢·*`.
PANE_PI_SPINNER_ASCII = """● Duas respostas — deixa eu confirmar a segunda

   Thinking…

 * Boondoggling… (thinking with high effort · 12s)

○ Todos (3/3)
├─ ✓ Marcar RESPONSAVEL_FALLBACK com badge onde renderiza
└─ ✓ Atualizar MOCKS.md com o que a auditoria encontrar

────────────────────────────────────────────────────────────
"""


def test_previa_para_no_spinner_ascii_e_nao_engole_os_todos():
    out = extract_assistant_text(PANE_PI_SPINNER_ASCII, "pi")
    assert out.startswith("Duas respostas")
    assert "Boondoggling" not in out
    assert "Todos (3/3)" not in out and "RESPONSAVEL_FALLBACK" not in out


def test_bullet_de_markdown_com_asterisco_nao_trunca_a_previa():
    # O corte olha a FORMA da linha (termina em `…` ou `)`), nao so o `*`: uma lista em prosa
    # continua sendo parte do bloco.
    pane = "● Ficou assim:\n* primeiro item\n* segundo item\n\n✻ Unfurling… (2s)\n"
    out = extract_assistant_text(pane, "pi")
    assert "primeiro item" in out and "segundo item" in out


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


PANE_SUBAGENTE_AO_VIVO = (
    "   Thought for 2s\n"
    "\n"
    " ● Achei — csharp-reviewer (user agent). Rodando no diff do commit:\n"
    "\n"
    + "─" * 40 + "\n"
    " ● Subagent Subagent (1 line)\n"
    "  └ The code patterns look clean. Now let me verify compilation.\n"
    + "─" * 40 + "\n"
    "\n"
    " Tool output: collapsed\n"
    + "─" * 40 + "\n"
)


def test_painel_do_subagente_AO_VIVO_nao_vira_previa():
    # Medido no pane em 03/08/2026: este painel usa o MESMO ● da prosa, fica DENTRO do corte da
    # ultima regua (o corte por posicao nao alcanca) e o subagente reescreve o `└` a cada frame --
    # a previa trocava de altura sem parar e a conversa pulava. A previa tem que cair na ultima
    # PROSA; ela ja esta no .jsonl, entao o preview_is_committed a suprime e nao sobra nada pulando.
    out = extract_assistant_text(PANE_SUBAGENTE_AO_VIVO)
    assert out == "Achei — csharp-reviewer (user agent). Rodando no diff do commit:"


def test_prosa_comecando_com_subagent_continua_sendo_prosa():
    # Falso positivo aqui DESCARTA o bloco e a previa fica VAZIA -- pior que vir suja. Por isso o
    # cabecalho sozinho nao basta: sem o `└` embaixo, e prosa.
    pane = (
        " ● Subagent driven development e o proximo passo aqui.\n"
        "   Vou explicar por que.\n"
        + "─" * 40 + "\n"
    )
    assert extract_assistant_text(pane).startswith("Subagent driven development")


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


# ── Previa vinda do AGENTE (sidecar) em vez do pane ────────────────────────────────────────────
# Contrato: a extensao do Pi publica o bloco em voo em <config>/.claude-pocket-preview/<stem>.json.
# O pane vira plano B — sessao sem a extensao (ou aberta antes dela) nao pode ficar sem previa.

def _sidecar(tmp_path, monkeypatch, payload, stem="s1"):
    import json as _json
    from app import preview as _prev
    d = tmp_path / ".claude-pocket-preview"
    d.mkdir(parents=True, exist_ok=True)
    if payload is not None:
        (d / f"{stem}.json").write_text(_json.dumps(payload), encoding="utf-8")
    monkeypatch.setattr(_prev, "_config_dirs", lambda: [tmp_path])
    return stem


def test_sidecar_devolve_o_texto_publicado(tmp_path, monkeypatch):
    import time as _t
    from app.preview import read_sidecar
    stem = _sidecar(tmp_path, monkeypatch, {"text": "texto em voo", "ts": _t.time()})
    assert read_sidecar(stem) == "texto em voo"


def test_sidecar_vazio_e_resposta_nao_ausencia(tmp_path, monkeypatch):
    # "" = o agente disse que NAO ha nada em voo (turno fechou). Cair no pane aqui traria de volta o
    # bloco ja commitado -> bolha duplicada.
    import time as _t
    from app.preview import read_sidecar
    stem = _sidecar(tmp_path, monkeypatch, {"text": "", "ts": _t.time()})
    assert read_sidecar(stem) == ""


def test_sidecar_ausente_ou_velho_cai_no_pane(tmp_path, monkeypatch):
    import time as _t
    from app.preview import read_sidecar
    assert read_sidecar(_sidecar(tmp_path, monkeypatch, None)) is None          # sem arquivo
    assert read_sidecar(_sidecar(tmp_path, monkeypatch,
                                 {"text": "antigo", "ts": _t.time() - 10_000})) is None
    assert read_sidecar(None) is None                                           # sessao sem stem


def test_sidecar_de_tipo_errado_nao_derruba_nada(tmp_path, monkeypatch):
    # JSON valido do tipo errado nao levanta ValueError: sem o guard o .get() explodia dentro do
    # loop do broker (mesmo acidente que ja derrubou a resolucao de estado pela statusline).
    from app.preview import read_sidecar
    d = tmp_path / ".claude-pocket-preview"
    d.mkdir(parents=True)
    (d / "s1.json").write_text("null", encoding="utf-8")
    (d / "s2.json").write_text("{isso nao e json", encoding="utf-8")
    from app import preview as _prev
    monkeypatch.setattr(_prev, "_config_dirs", lambda: [tmp_path])
    assert read_sidecar("s1") is None and read_sidecar("s2") is None


def test_broker_prefere_o_sidecar_e_nao_le_o_pane(tmp_path, monkeypatch):
    # O ganho principal: com o agente publicando, o capture-pane (um subprocess a cada 150ms por
    # sessao) nem roda.
    import asyncio as _aio, time as _t
    from app import preview as _prev
    stem = _sidecar(tmp_path, monkeypatch, {"text": "veio do agente", "ts": _t.time()})
    chamou = []
    monkeypatch.setattr(_prev.tmux, "capture_pane", lambda n: chamou.append(n) or "● veio do pane")

    # subscribe() emite o slot ATUAL de cara (vazio, antes do 1o poll) -- ele nao e resposta, so o
    # estado inicial. O que interessa e o 1o texto de verdade.
    async def roda():
        b = _prev.PreviewBroker("sessao-x", "pi", lambda: stem)
        agen = b.subscribe()
        try:
            async def primeiro_nao_vazio():
                async for t, md in agen:
                    if t:
                        return t, md
            return await _aio.wait_for(primeiro_nao_vazio(), 2)
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-x", None)

    # O par (texto, md) sai JUNTO da fonte: md=True marca "markdown cru, a bolha renderiza".
    assert _aio.run(roda()) == ("veio do agente", True)
    assert chamou == [], "leu o pane mesmo com sidecar publicado"


def test_broker_sem_sidecar_continua_no_pane(tmp_path, monkeypatch):
    import asyncio as _aio
    from app import preview as _prev
    _sidecar(tmp_path, monkeypatch, None)
    monkeypatch.setattr(_prev.tmux, "capture_pane", lambda n: "● veio do pane\n\n✻ Unfurling… (2s)\n")

    async def roda():
        b = _prev.PreviewBroker("sessao-y", "pi", lambda: "s1")
        agen = b.subscribe()
        try:
            async def primeiro_nao_vazio():
                async for t, md in agen:
                    if t:
                        return t, md
            return await _aio.wait_for(primeiro_nao_vazio(), 2)
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-y", None)

    assert _aio.run(roda()) == ("veio do pane", False)   # raspado da TUI: ja pintado, nao renderiza


def test_broker_segue_o_stem_da_conexao_mais_recente():
    # Achado da review: o broker e singleton por sessao, mas a closure do stem vem da CONEXAO. Se o
    # 1o subscriber cai e outro segue vivo (celular + desktop), travar no primeiro deixaria o broker
    # lendo o stem da sessao ANTERIOR depois de um /clear -> previa da sessao nova com texto da velha.
    from app import preview as _prev
    try:
        b1 = _prev.PreviewBroker.get("sessao-z", "pi", lambda: "stem-velho")
        b2 = _prev.PreviewBroker.get("sessao-z", "pi", lambda: "stem-novo")
        assert b1 is b2                          # segue UM broker por sessao
        assert b2.stem_get() == "stem-novo"
        # Conexao sem stem_get (Claude/Codex) nao pode APAGAR o de quem tem.
        assert _prev.PreviewBroker.get("sessao-z", "pi").stem_get() == "stem-novo"
    finally:
        _prev.PreviewBroker._brokers.pop("sessao-z", None)

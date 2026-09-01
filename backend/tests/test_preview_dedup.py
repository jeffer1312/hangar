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
  🤖 Opus5·1M (high✦) │ 📁 hangar [main*] │ 💵 $10.51
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
# Contrato: a extensao do Pi publica o bloco em voo em <config>/.hangar-preview/<stem>.json.
# O pane vira plano B — sessao sem a extensao (ou aberta antes dela) nao pode ficar sem previa.

def _sidecar(tmp_path, monkeypatch, payload, stem="s1"):
    import json as _json
    from app import preview as _prev
    d = tmp_path / ".hangar-preview"
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
    # "" velho NAO envelhece: e a resposta "nada em voo" do fim do turno. Descarta-lo mandava o
    # broker raspar o pane parado e o ultimo bloco commitado virava bolha fantasma ao reabrir o app.
    assert read_sidecar(_sidecar(tmp_path, monkeypatch,
                                 {"text": "", "ts": _t.time() - 10_000})) == ""


def test_sidecar_de_tipo_errado_nao_derruba_nada(tmp_path, monkeypatch):
    # JSON valido do tipo errado nao levanta ValueError: sem o guard o .get() explodia dentro do
    # loop do broker (mesmo acidente que ja derrubou a resolucao de estado pela statusline).
    from app.preview import read_sidecar
    d = tmp_path / ".hangar-preview"
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
                async for t, md, full in agen:
                    if t:
                        return t, md, full
            return await _aio.wait_for(primeiro_nao_vazio(), 2)
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-x", None)

    # O trio (texto, md, full) sai JUNTO da fonte: md=True marca "markdown cru, a bolha renderiza",
    # full=True marca "incremental" (o agente publica na ordem -> a bolha fica sem teto).
    assert _aio.run(roda()) == ("veio do agente", True, True)
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
                async for t, md, full in agen:
                    if t:
                        return t, md, full
            return await _aio.wait_for(primeiro_nao_vazio(), 2)
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-y", None)

    # raspado da TUI: ja pintado, nao renderiza; e troca inteira (nao incremental) -> teto de 10 linhas
    assert _aio.run(roda()) == ("veio do pane", False, False)


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


def _pane_kimi_eco_e_dica() -> str:
    return (Path(__file__).parent / "fixtures" / "pane_kimi_eco_e_dica.txt").read_text(
        encoding="utf-8")


def test_preview_kimi_nao_vaza_eco_do_prompt_nem_a_dica():
    """O pane do Kimi guarda DUAS coisas entre a resposta e a caixa do composer, e as duas vazavam
    pra dentro da bolha do assistente (relatado com print do app em 11/08/2026):

        …prosa do assistente…
         ✨ sim                                        <- o proprio prompt do usuario, ecoado
          🌓 · Tip: Try /dance for a hidden Easter egg  <- rodape de dica

    O Kimi herdou a config do Pi (`_PI_BOX_RE` como unica parada) porque ele desenha a MESMA caixa
    arredondada. Verdade — mas a caixa fica ABAIXO das duas linhas, entao a parada chegava tarde.

    Fixture = captura real do pane (150 quadros a 1/s), nao texto inventado: as duas linhas trazem
    glifo ANIMADO (8 fases da lua parado, ciclo braille em voo) e escrever a regra olhando print
    fixaria um quadro so.
    """
    out = extract_assistant_text(_pane_kimi_eco_e_dica(), "kimi")
    assert "✨" not in out, "o prompt do usuario voltou dentro da resposta do assistente"
    assert "Tip:" not in out, "o rodape de dica do TUI entrou na resposta"
    # e a prosa de verdade continua inteira — parada cedo demais seria a troca de um bug por outro
    assert "Quer que eu faça?" in out
    assert "sincroniza só para o Claude" in out


def test_preview_kimi_corta_a_dica_nas_DUAS_formas():
    """Parado a linha comeca com fase da lua; em voo, com spinner braille + "working...". Uma regra
    que so cobrisse a forma do print (a lua) deixaria o vazamento vivo durante o turno, que e
    justamente quando o preview ao vivo existe."""
    for chrome in ("  🌓 · Tip: Try /dance for a hidden Easter egg",
                   "  ⠋ working... · Tip: /goal for multi-step work with a clear finish line"):
        pane = "● resposta do assistente\n\n" + chrome + "\n ╭───────────╮\n │ >         │\n ╰───────────╯"
        out = extract_assistant_text(pane, "kimi")
        assert out == "resposta do assistente", f"nao cortou: {out!r}"


# ── Raciocinio do Kimi vazando na previa (14/08/2026) ─────────────────────────────────────────
# O usuario viu o RASCUNHO do Kimi aparecer no chat como se fosse a mensagem. O transcript nunca
# teve isso (o parser descarta `part.type == "think"`); quem vazava era a PREVIA, que pro Kimi le o
# pane — e la o raciocinio e a resposta usam o MESMO `●`, so mudando a cor. Bytes abaixo copiados de
# um `capture-pane -e` real (Kimi 0.36, K3 high): cinza 136 + ITALICO no rascunho, claro 224 sem
# italico na resposta.
_KIMI_PENSANDO = (
    " \x1b[38;2;136;136;136m● \x1b[3mCount letter r in \"strawberry raspberry blueberry\".\x1b[0m\n"
    "   \x1b[2m... (9 more lines, ctrl+o to expand)\x1b[0m\n"
    "\n"
    "  \x1b[38;2;136;136;136m⠋ working...\x1b[0m\n"
)
_KIMI_RESPONDENDO = (
    " \x1b[38;2;224;224;224m● \x1b[39mSao 6 letras r no total.\n"
)
# Resposta REAL com enfase de markdown no meio (`*teste*` -> italico), capturada do mesmo jeito. A
# primeira versao do filtro cortava esta linha inteira: ela TEM `\x1b[3m`. Os dois reviewers
# levantaram a hipotese e a captura confirmou — por isso a regra e posicao do italico + cor, nao "a
# linha tem italico".
_KIMI_RESPOSTA_COM_ITALICO = (
    " \x1b[38;2;224;224;224m● \x1b[39mO \x1b[3mteste\x1b[0m \x1b[1mpassou\x1b[0m com sucesso.\n"
)


def test_kimi_raciocinio_sai_da_previa():
    from app.preview import sem_pensamento_kimi
    limpo = sem_pensamento_kimi(_KIMI_PENSANDO)
    assert "Count letter r" not in limpo          # o rascunho sai
    assert "more lines, ctrl+o" not in limpo      # e o rodape do bloco dobrado junto
    assert "\x1b[" not in limpo                   # devolve texto puro (o resto do modulo casa isso)
    assert extract_assistant_text(limpo, "kimi") == ""


def test_kimi_resposta_continua_na_previa():
    from app.preview import sem_pensamento_kimi
    limpo = sem_pensamento_kimi(_KIMI_PENSANDO + _KIMI_RESPONDENDO)
    assert extract_assistant_text(limpo, "kimi") == "Sao 6 letras r no total."


def test_kimi_sem_cor_nao_perde_texto():
    # Pane sem ANSI (dublê de teste, ou `capture-pane` sem `-e` por engano): nada tem italico, entao
    # nada e descartado — degrada pro comportamento de antes, nunca engole a resposta.
    from app.preview import sem_pensamento_kimi
    puro = "● Sao 6 letras r no total.\n"
    assert sem_pensamento_kimi(puro) == puro.rstrip("\n")


def test_kimi_enfase_na_resposta_nao_e_confundida_com_rascunho():
    from app.preview import sem_pensamento_kimi
    limpo = sem_pensamento_kimi(_KIMI_PENSANDO + _KIMI_RESPOSTA_COM_ITALICO)
    assert extract_assistant_text(limpo, "kimi") == "O teste passou com sucesso."


# Status de ferramenta concluida do Kimi, copiado do print do usuario (19/08/2026): a TUI desenha
# "● Used <Tool> (…)" com o MESMO ● da prosa, e o "Used" nao estava em nenhuma lista — a linha era
# eleita como bloco em voo e a previa mostrava o status como texto, quando o ToolCard da chamada ja
# chega pelo tool.call do wire.
_KIMI_USED = (
    "● Used ReadMediaFile (…rojetos/hangar/.hangar-uploads/1787141544-3efdfc.png)"
    " · image (image/jpeg, 49.1 KB)\n"
)


def test_kimi_used_nao_e_prosa():
    # So o status no pane: nenhum bloco de prosa -> previa vazia (o ToolCard vem do wire).
    assert extract_assistant_text(_KIMI_USED, "kimi") == ""


def test_kimi_used_depois_da_prosa_corta_o_bloco():
    pane = "● A resposta comeca aqui.\n" + _KIMI_USED
    assert extract_assistant_text(pane, "kimi") == "A resposta comeca aqui."


# Ferramenta EM EXECUCAO, copiada do pane real (24/08/2026, 321 amostras iguais): a TUI desenha
# "● Using <Tool> (caminho)" e, embaixo, o conteudo do arquivo com numero de linha.
_KIMI_USING = (
    "● Using Write (docs/superpowers/plans/2026-08-24-folha-de-exames.md)\n"
    "      1  # Folha de exames — Implementation Plan\n"
)


def test_kimi_using_nao_e_prosa():
    assert extract_assistant_text(_KIMI_USING, "kimi") == ""


def test_kimi_using_depois_da_prosa_corta_o_bloco():
    # O caso do print do usuario: sem o corte, a previa virava a prosa MAIS o rotulo da TUI e o
    # arquivo inteiro numerado.
    pane = "● A resposta comeca aqui.\n" + _KIMI_USING
    assert extract_assistant_text(pane, "kimi") == "A resposta comeca aqui."


def test_kimi_using_em_prosa_inglesa_nao_descarta():
    # Mesma amarra do "Used": em prosa a palavra seguinte e minuscula e nao ha parentese colado.
    pane = "● Using it twice made the render flicker.\n"
    assert extract_assistant_text(pane, "kimi") == "Using it twice made the render flicker."


def test_kimi_used_em_prosa_inglesa_nao_descarta():
    # "Used" em prosa de verdade tem a palavra seguinte minuscula — NAO pode descartar o bloco
    # (previa vazia e pior que previa suja, regra do modulo).
    pane = "● Used correctly, the flag avoids the double render.\n"
    assert extract_assistant_text(pane, "kimi") == "Used correctly, the flag avoids the double render."


def test_kimi_previa_do_broker_ja_vem_sem_rascunho(monkeypatch):
    """Fiacao fim a fim: provider kimi -> capture_pane(cores=True) -> sem_pensamento_kimi.

    Os testes acima exercitam as pecas soltas; este prova que elas estao LIGADAS. Sem ele, trocar a
    ordem dos argumentos do capture_pane viraria TypeError dentro do `to_thread`, engolido pelo
    `except Exception: pane = ""` do _loop — previa muda, indistinguivel de pane vazio.
    """
    import asyncio as _aio
    from app import preview as _prev

    vistos = []

    def falso_capture(name, lines=200, cores=False):
        vistos.append((name, lines, cores))
        return _KIMI_PENSANDO + _KIMI_RESPONDENDO

    monkeypatch.setattr(_prev.tmux, "capture_pane", falso_capture)
    monkeypatch.setattr(_prev, "read_sidecar", lambda stem: None)

    async def roda():
        b = _prev.PreviewBroker("sessao-kimi", "kimi", lambda: "s-kimi")
        agen = b.subscribe()
        try:
            async def primeiro_nao_vazio():
                async for t, _md, _full in agen:
                    if t:
                        return t
            return await _aio.wait_for(primeiro_nao_vazio(), 2)
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-kimi", None)

    assert _aio.run(roda()) == "Sao 6 letras r no total."
    assert vistos and vistos[0][2] is True      # pediu o pane COM cor


# ── Costura do texto em voo do Kimi (_costurar) ──────────────────────────────────────────────────
# A TUI do Kimi roda em tela alternativa (alternate_on=1, medido em 19/08/2026): o que sobe da
# janela visivel SE PERDE do pane. O broker le a cada 150ms e cola os quadros pela sobreposicao —
# sem isto o comeco de uma resposta longa so aparecia depois do commit.

def test_costura_cresce_sem_rolar():
    from app.preview import _costurar
    acum, colou = _costurar("", "Primeira frase.")
    assert (acum, colou) == ("Primeira frase.", True)
    # Nada rolou pra fora da tela: o quadro contem o texto inteiro -> substitui (nao duplica).
    assert _costurar(acum, "Primeira frase. Segunda frase.") == ("Primeira frase. Segunda frase.", True)


def test_costura_cola_pela_sobreposicao_quando_o_topo_some():
    from app.preview import _costurar
    # O topo ("Primeira frase comprida. ") saiu da janela visivel; o quadro novo traz so a cauda +
    # a linha nova. A sobreposicao real entre dois quadros a 150ms e quase a janela inteira — o
    # piso de 24 chars e folga, nao aperto.
    acum = _costurar("Primeira frase comprida pra valer. Segunda frase comprida pra valer.",
                     "Segunda frase comprida pra valer. Terceira frase, recem-chegada.")
    assert acum == ("Primeira frase comprida pra valer. Segunda frase comprida pra valer."
                    " Terceira frase, recem-chegada.", True)


def test_costura_quadro_vazio_mantem_o_acumulado():
    from app.preview import _costurar
    # Ferramenta rodando / spinner: o extrator nao acha prosa -> "". O bloco em voo CONTINUA —
    # zerar aqui apagaria o que ja foi costurado ate a prosa voltar.
    assert _costurar("texto acumulado", "") == ("texto acumulado", True)


def test_costura_sem_sobreposicao_recomeca():
    from app.preview import _costurar
    # Bloco NOVO (o part anterior commitou e outro comecou): sem overlap confiavel -> recomeca,
    # em vez de grudar uma resposta na outra.
    acum = _costurar("Resposta da primeira pergunta, completa.", "Assunto totalmente diferente agora.")
    # colou=False: o texto TROCOU inteiro — e o que mantem a flag full do broker honesta (o frame
    # de recomeco NAO e "so cresce no fim", entao a bolha fica com o teto nesse frame).
    assert acum == ("Assunto totalmente diferente agora.", False)


def test_costura_overlap_curto_demais_e_coincidencia():
    from app.preview import _costurar
    # Sufixo/prefixo casam por acidente ("a") mas menos que _COSTURA_MIN: NAO cola — e bloco novo.
    acum = _costurar("Termina com a", "a resposta nova comeca aqui de verdade")
    assert acum == ("a resposta nova comeca aqui de verdade", False)


def test_kimi_previa_do_broker_costura_os_quadros(monkeypatch):
    """Fiacao fim a fim da costura: quadros sequenciais do pane — o ultimo JA SEM a linha do ●,
    que rolou pra fora da janela (medido nos quadros reais de 19/08/2026) — e o broker publica o
    texto INTEIRO, com full=True (a flag que tira o teto de 10 linhas no front)."""
    import asyncio as _aio
    from app import preview as _prev

    quadros = iter([
        "● Primeira frase comprida pra valer.\n",
        "● Primeira frase comprida pra valer. Segunda frase comprida pra valer.\n",
        # O ● e o topo rolaram pra fora da janela: o quadro so tem a cauda + a linha nova, e quem
        # recupera e a extracao de CONTINUACAO (_extrair_continuacao_kimi) + a costura.
        "Segunda frase comprida pra valer. Terceira frase, recem-chegada.\n",
    ])

    monkeypatch.setattr(_prev.tmux, "capture_pane", lambda name, lines=200, cores=False: next(quadros, ""))
    monkeypatch.setattr(_prev, "read_sidecar", lambda stem: None)

    async def roda():
        b = _prev.PreviewBroker("sessao-kimi-costura", "kimi", lambda: "s-kimi")
        agen = b.subscribe()
        vistos = []
        try:
            async def coleta():
                async for t, md, full in agen:
                    if t:
                        vistos.append((t, md, full))
                    if len(vistos) >= 3:
                        return vistos
            # Cadencia ociosa (sem spinner nos quadros falsos): 0.75s por poll -> 3 textos levam
            # ~2.3s; 4s de teto com folga.
            return await _aio.wait_for(coleta(), 4)
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-kimi-costura", None)

    vistos = _aio.run(roda())
    assert vistos[-1][0] == ("Primeira frase comprida pra valer. Segunda frase comprida pra valer."
                             " Terceira frase, recem-chegada.")
    assert vistos[-1][1] is False    # texto do pane: plano, nao renderiza markdown
    assert vistos[-1][2] is True     # costurado = incremental -> a bolha fica sem o teto


def test_kimi_broker_rejeita_continuacao_que_nao_cola(monkeypatch):
    """O portao declarado do broker: janela cheia de SAIDA DE FERRAMENTA (nada do bloco em voo) —
    a extracao de continuacao devolve texto, mas a costura nao cola e o ACUMULADO fica intacto.
    Sem este teste o portao e so comentario (achado da review)."""
    import asyncio as _aio
    from app import preview as _prev

    quadros = iter([
        "● Bloco em voo, primeira frase comprida pra valer.\n",
        # Ferramenta rodando: a janela agora e so saida de comando — nada cola com o acumulado.
        "total 48\n-rw-r--r-- 1 user user 9131 ago 19 09:41 preview.py\n",
    ])

    monkeypatch.setattr(_prev.tmux, "capture_pane", lambda name, lines=200, cores=False: next(quadros, ""))
    monkeypatch.setattr(_prev, "read_sidecar", lambda stem: None)

    async def roda():
        b = _prev.PreviewBroker("sessao-kimi-portao", "kimi", lambda: "s-kimi")
        agen = b.subscribe()
        try:
            async def primeiro():
                async for t, _md, _full in agen:
                    if t:
                        return t
            primeiro_texto = await _aio.wait_for(primeiro(), 4)
            # Espera o 2o quadro rodar (cadencia ociosa 0.75s) e confere o estado INTERNO:
            # o acumulado nao pode ter virado a saida da ferramenta.
            await _aio.sleep(1.2)
            return primeiro_texto, b._kimi_acum
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-kimi-portao", None)

    texto, acum = _aio.run(roda())
    assert texto == "Bloco em voo, primeira frase comprida pra valer."
    assert acum == texto                    # a saida da ferramenta NAO entrou
    assert "preview.py" not in acum


def test_kimi_broker_reset_limpa_texto_e_acumulado(monkeypatch):
    """O /clear trocou de transcript: reset() zera texto e acumulado pra reconexao nao receber a
    conversa APAGADA no primeiro yield (achado da review — com a supressao desarmada, era bolha
    fantasma com full=True)."""
    import asyncio as _aio
    from app import preview as _prev

    monkeypatch.setattr(_prev.tmux, "capture_pane",
                        lambda name, lines=200, cores=False: "● Texto de antes do clear, em voo.\n")
    monkeypatch.setattr(_prev, "read_sidecar", lambda stem: None)

    async def roda():
        b = _prev.PreviewBroker("sessao-kimi-reset", "kimi", lambda: "s-kimi")
        agen = b.subscribe()
        try:
            async def primeiro():
                async for t, _md, _full in agen:
                    if t:
                        return t
            assert await _aio.wait_for(primeiro(), 4) == "Texto de antes do clear, em voo."
            assert b._kimi_acum                              # acumulado de verdade, nao so slot
            gen_antes = b._gen
            b.reset()
            assert b.text == "" and b._kimi_acum == "" and b.full is False and b.md is False
            assert b._gen == gen_antes + 1                   # epoca avancou (frame em voo morre)
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-kimi-reset", None)

    _aio.run(roda())


def test_kimi_broker_descarta_frame_capturado_antes_do_reset(monkeypatch):
    """A corrida da epoca: o /clear (reset) cai no MEIO do capture — o frame voltou do to_thread
    com texto da conversa apagada, e nem publica nem deixa o acumulado renascer. Sem a epoca, o
    fantasma vivia ate o proximo bloco de prosa (achado da review)."""
    import asyncio as _aio
    from app import preview as _prev

    broker_ref = []
    chamadas = {"n": 0}

    def falso_capture(name, lines=200, cores=False):
        chamadas["n"] += 1
        if chamadas["n"] == 1:
            broker_ref[0].reset()        # o /clear cai no MEIO do capture
            return "● Texto da conversa apagada, ainda na tela.\n"
        return ""                        # depois o pane limpou de verdade

    monkeypatch.setattr(_prev.tmux, "capture_pane", falso_capture)
    monkeypatch.setattr(_prev, "read_sidecar", lambda stem: None)

    async def roda():
        b = _prev.PreviewBroker("sessao-kimi-epoca", "kimi", lambda: "s-kimi")
        broker_ref.append(b)
        agen = b.subscribe()
        publicados = []
        try:
            async def coleta():
                async for t, _md, _full in agen:
                    if t:
                        publicados.append(t)
            task = _aio.create_task(coleta())
            await _aio.sleep(1.5)        # ~2 polls ociosos: tempo de sobra pro frame voltar
            task.cancel()
            try:
                await task               # espera o cancel morder antes do aclose (senao o
            except _aio.CancelledError:  # generator ainda esta rodando e o aclose levanta)
                pass
            return publicados, b
        finally:
            await agen.aclose()
            _prev.PreviewBroker._brokers.pop("sessao-kimi-epoca", None)

    publicados, b = _aio.run(roda())
    assert publicados == []              # nada da conversa apagada escapou
    assert b._kimi_acum == ""            # e o acumulado nao renasceu


# Painel de Todo da TUI do Kimi, desenho medido nos quadros reais de 19/08/2026: header "Todo",
# itens "✓"/"●", entre a regua e a caixa do composer — o item atual usa o MESMO ● da prosa e era
# eleito bloco em voo (a previa mostrava o item no lugar do texto).
_PANE_KIMI_TODO = (
    " ● Enquanto a captura roda, vou aproveitar pra deixar registrado o desenho completo.\n"
    "   O texto continua aqui, segundo paragrafo do bloco em voo.\n"
    "\n"
    "  ⠸ working... · Tip: Try /dance for a hidden Easter egg\n"
    " ─────────────────────────────────────────────────────────────────────────\n"
    "   Todo\n"
    "   ✓ Backend: costura (_costurar) + acumulador no PreviewBroker (só kimi)\n"
    "   ● Testes backend (costura, broker, unpacks) e suíte verde\n"
    " ╭───────────────────────────────────────────────────────────────────────╮\n"
    " │ >                                                                     │\n"
    " ╰───────────────────────────────────────────────────────────────────────╯\n"
)


def test_kimi_painel_de_todo_nao_vira_prosa():
    out = extract_assistant_text(_PANE_KIMI_TODO, "kimi")
    assert "Testes backend" not in out                       # o item do painel nao e prosa
    assert out.startswith("Enquanto a captura roda")         # e a prosa em voo continua dona
    assert "segundo paragrafo" in out


def test_kimi_spinner_sem_dica_para_a_continuacao():
    # Variante medida na fixture _KIMI_PENSANDO: "  ⠋ working..." SOZINHO (sem " · Tip:"). Sem a
    # parada por glifo, a prosa engolia spinner + painel de Todo ate a caixa do composer.
    pane = _PANE_KIMI_TODO.replace("⠸ working... · Tip: Try /dance for a hidden Easter egg",
                                   "⠋ working...")
    out = extract_assistant_text(pane, "kimi")
    assert "working" not in out
    assert "Testes backend" not in out
    assert "segundo paragrafo" in out


def test_kimi_continuacao_sem_bullet_pega_o_topo_ate_o_chrome():
    from app.preview import _extrair_continuacao_kimi
    # Bloco estourou a janela: o ● sumiu, o topo e o MEIO do bloco. A funcao devolve tudo ate o
    # chrome — quem decide se cola e a costura.
    pane = ("   meio do paragrafo que estava rolando.\n"
            "   A ideia. O PreviewBroker le o pane a cada 150ms.\n"
            "\n"
            "  ⠸ working... · Tip: Try /dance\n"
            " ╭───────────────────────────────────────────────────────────╮\n")
    out = _extrair_continuacao_kimi(pane)
    assert out == ("   meio do paragrafo que estava rolando.\n"
                   "   A ideia. O PreviewBroker le o pane a cada 150ms.")


def test_kimi_continuacao_nao_aceita_saida_de_ferramenta():
    from app.preview import _costurar, _extrair_continuacao_kimi
    # Janela cheia de saida de ferramenta (nada do bloco em voo): a extracao devolve texto, mas a
    # costura NAO cola (sem overlap com o acumulado) — e o broker, nesse caso, mantem o acumulado.
    pane = ("total 48\n"
            "-rw-r--r-- 1 jefferson jefferson  9131 ago 19 09:41 preview.py\n"
            "  ⠸ working... · Tip: x\n")
    cont = _extrair_continuacao_kimi(pane)
    # Nao cola: a costura devolve o proprio quadro com colou=False — e o broker, nesse caso,
    # mantem o acumulado (a decisao dele; testada fim a fim no teste do broker abaixo).
    assert _costurar("Primeira frase comprida pra valer, ainda em voo.", cont) == (cont, False)



# ── Chrome novo do Claude Code: aviso de subagente concluido + resumo de atividade ────────────────
# Desenho copiado do pane REAL (17/08/2026, claude 2.1.233) — o print do usuario, reproduzido:
# o aviso "Agent ... finished" usa o MESMO ● da prosa e era eleito bloco em voo; o resumo
# ("Searched for N patterns, ..., ran N shell commands") entrava como continuacao da prosa.

PANE_AGENT_FINISHED = """● Os dois reviewers voltaram OK, repetindo o push agora.

✻ Waiting for 1 background agent to finish

● Agent "react-reviewer no diff" finished · 9s

  Searched for 2 patterns, read 1 file, listed 1 directory, ran 2 shell commands
"""


def test_aviso_de_agente_concluido_nao_e_eleito_previa():
    out = extract_assistant_text(PANE_AGENT_FINISHED)
    assert out == "Os dois reviewers voltaram OK, repetindo o push agora."


def test_resumo_de_atividade_nao_gruda_na_prosa():
    pane = "\n".join([
        "● Repetindo o comando identico agora:",
        "",
        "  Pushed to minha-branch, ran 3 shell commands ",
    ])
    assert extract_assistant_text(pane) == "Repetindo o comando identico agora:"


def test_prosa_citando_shell_commands_no_meio_continua_inteira():
    # a ancora exige a FORMA no fim da linha — mencao no meio da frase e prosa
    pane = "\n".join([
        "● O hook ran 3 shell commands antes de travar, e o quarto",
        "  ficou pendente na fila.",
    ])
    assert extract_assistant_text(pane).endswith("ficou pendente na fila.")

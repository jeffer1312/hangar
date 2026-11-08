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


def test_suprime_lista_pintada_com_bullet_pela_tui():
    # A TUI (medido no Kimi) desenha `- item` como `• item` e hard-wrapa: ao abrir a sessão, o pane
    # mostra o último bloco já commitado e a comparação falhava no primeiro marcador de lista.
    commitado = ("Sim, tem — e está em dois lugares:\n\n"
                 "- `docs/polish-backlog.md:669` — o item pendente em si, na seção de dívida\n"
                 "- `CLAUDE.md:221-235` — o aviso operacional que aponta pra lá")
    pane = ("Sim, tem — e está em dois lugares:\n\n"
            "   • docs/polish-backlog.md:669 — o item pendente em si, na seção de\n"
            "     dívida\n"
            "   • CLAUDE.md:221-235 — o aviso operacional que aponta pra lá")
    assert preview_is_committed(pane, _norm(commitado))


def test_hifen_de_prosa_continua_distinguindo_texto():
    # Só o marcador no início da linha sai: um bloco novo que difere do commitado por um hífen
    # dentro da palavra não pode ser engolido como "já commitado".
    commitado = "O envio por e-mail falhou porque o servidor recusou a conexão hoje."
    novo = "O envio por email falhou porque o servidor recusou a conexão hoje."
    assert not preview_is_committed(novo, _norm(commitado))


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


def test_chamada_em_voo_com_o_ponto_aceso_ou_apagado_da_a_mesma_previa():
    """O ● da chamada em voo pisca, e a 1ª linha dela é a descrição, não "Bash(".

    Aceso, a descrição era eleita prosa; apagado, grudava na prosa de cima. A prévia trocava a cada
    piscada e o bloco crescia e encolhia. Os dois quadros são capturas reais do Claude Code.
    """
    fx = Path(__file__).parent / "fixtures"
    esperado = "Primeiro comando concluído.\n\n  Aguardando o segundo..."
    for nome in ("pane_ferramenta_em_voo_acesa.txt", "pane_ferramenta_em_voo_apagada.txt"):
        assert extract_assistant_text((fx / nome).read_text(encoding="utf-8")) == esperado, nome


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


def test_preview_claude_ignora_a_regra_do_pi():
    # `└` na linha seguinte a um ● do Claude e prosa desenhando arvore: o marcador dele e `⎿`.
    pane = "● Ficou assim:\n └ dist/\n\n✻ Thinking… (2s)\n"
    assert extract_assistant_text(pane).startswith("Ficou assim:")


# Pane REAL (Pi 0.82.1 + k3, 03/08/2026) do frame que vazava: o spinner esta no quadro ASCII `*`
# (U+002A) -- fora de SPINNER_GLYPHS -- e por isso nao parava a varredura, que seguia engolindo a
# linha de status E o painel de Todos inteiro. Um frame em seis do ciclo `✻✽✶✺✢·*`.


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


PANE_AVISO_DE_LARGADA = (
    "  Claude Code v2.1.267\n"
    "\n"
    "⚠ CLAUDE.md is over the 150.0k-char limit (178.4k chars)\n"
    "\n"
    '● ecc: hooks.json: unknown keys "$schema", "description" in hooks.PreToolUse[0]\n'
    "  ignored\n"
    "\n"
    "❯ pq isso esta aparecendo no hangar?\n"
    "\n"
    "  Thought for 5s, read 2 files\n"
    "\n"
    + "─" * 40 + "\n"
    "❯ \n"
    + "─" * 40 + "\n"
)


def test_aviso_de_largada_nao_vira_previa():
    # O Claude Code imprime o aviso de largada com o MESMO ● da prosa. Numa sessao recem-aberta que
    # so rodou ferramenta, ele era o ultimo ● e virava previa. Corta pela ultima mensagem do usuario
    # -- prosa em voo e sempre posterior a ela.
    assert extract_assistant_text(PANE_AVISO_DE_LARGADA) == ""


def test_aviso_de_largada_antes_da_primeira_mensagem_nao_vira_previa():
    # Sessao recem-aberta, ainda sem NENHUM ❯ do usuario: o corte pela ultima mensagem nao tem onde
    # cortar e voltava a varrer tudo -- o aviso aparecia no chat vazio (medido no celular, 11/09).
    pane = (
        " ▐▛███▛█   Claude Code v2.1.268\n"
        "▝▜██████▀  Fable 5.1 with low effort · Claude Max\n"
        "\n"
        "⚠ CLAUDE.md is over the 150.0k-char limit (191.0k chars)\n"
        '● ecc: hooks.json: unknown keys "$schema", "description" in hooks.PreToolUse[0]\n'
        "  ignored\n"
        + "─" * 40 + "\n"
        "❯ \n"
        + "─" * 40 + "\n"
    )
    assert extract_assistant_text(pane) == ""


def test_aviso_de_plugin_reimpresso_no_meio_da_conversa_nao_vira_previa():
    # Pane real de 11/09 (sessão cux-rev-t3): o Claude Code reimprime o aviso do hooks.json DEPOIS
    # do último ❯, então nem o corte pela mensagem do usuário nem o do banner o alcançam.
    pane = (
        "❯ [de: hangar] 'cux-rev-t5' encerrou a sessão e saiu do grupo de trabalho.\n"
        "\n"
        "● Ciente — grupo agora é hangar, cux-t3 e eu. Aguardando a rodada 1 do cux-t3.\n"
        "\n"
        '● ecc: hooks.json: unknown keys "$schema", "description" in hooks.PreToolUse[0] and 40 more\n'
        "  ignored\n"
        "\n"
        + "─" * 40 + "\n"
        "❯ \n"
        + "─" * 40 + "\n"
    )
    # Vazio, e não o "Ciente…" anterior: aquele bloco já está no transcript, e reelegê-lo o
    # traria de volta como bolha em voo.
    assert extract_assistant_text(pane) == ""
    # Prosa DEPOIS do aviso continua sendo a prévia (o aviso pode vir no começo do turno).
    depois = pane.replace("  ignored\n", "  ignored\n\n● Agora sim, lendo o arquivo.\n")
    assert extract_assistant_text(depois) == "Agora sim, lendo o arquivo."
    # Prosa comum que começa parecida não é aviso.
    prosa = pane.replace("● ecc: hooks.json: unknown keys", "● Nota: hooks.json: define os hooks")
    assert extract_assistant_text(prosa).startswith("Nota: hooks.json: define")


def test_prosa_depois_da_mensagem_do_usuario_continua_sendo_previa():
    # O corte nao pode zerar o caso normal: o ❯ da caixa de digitar (regua/❯/regua) nao conta.
    pane = PANE_AVISO_DE_LARGADA.replace(
        "  Thought for 5s, read 2 files\n",
        "● Sao duas coisas. O aviso vem do plugin ecc.\n",
    )
    assert extract_assistant_text(pane) == "Sao duas coisas. O aviso vem do plugin ecc."


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


def test_sidecar_velho_com_sessao_trabalhando_segue_valendo(tmp_path, monkeypatch):
    # Turno longo só de ferramentas não publica texto novo: cair no pane mostrava como mensagem a
    # linha que a TUI desenha ao lado do spinner. Parada, a sessão velha continua caindo no pane.
    import time as _t
    from app.hook_state import hook_state
    from app.preview import read_sidecar
    stem = _sidecar(tmp_path, monkeypatch, {"text": "antigo", "ts": _t.time() - 10_000})
    monkeypatch.setitem(hook_state._map, stem, ("working", _t.time()))
    assert read_sidecar(stem) == "antigo"
    monkeypatch.setitem(hook_state._map, stem, ("idle", _t.time()))
    assert read_sidecar(stem) is None
    # Marcador "working" velho = agente morreu no meio do turno: não pode congelar a prévia.
    monkeypatch.setitem(hook_state._map, stem, ("working", _t.time() - 10_000))
    assert read_sidecar(stem) is None


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


def test_faixa_de_mod_com_bullet_nao_vira_previa():
    # A faixa que um mod desenha acima do prompt pode começar com ● (o de progresso começa) e ficava
    # eleita a prosa em andamento. A âncora é o primeiro texto da árvore que o plugin mandou.
    from app import plugin_bridge as pb

    pb._guardar_faixa("faixa", {"type": "Box", "children": [
        {"type": "Text", "children": ["● "]}, {"type": "Text", "children": ["Migração do relatório"]}]}, None, [])
    try:
        pane = "\n".join([
            "❯ faça tal coisa",
            "● Resposta em andamento",
            "",
            "● Migração do relatório                      2/6 passos",
            "  ━━━━━━━━━━━━━━━━━━━━━━━━━  33%",
            "─" * 40,
            "❯ ",
            "─" * 40,
        ])
        assert extract_assistant_text(pane).startswith("Migração do relatório")
        assert extract_assistant_text(pane, "claude", pb.band_anchor("faixa")) == "Resposta em andamento"
    finally:
        pb.esquecer("faixa")


def test_painel_ancorado_nao_entra_na_previa():
    from app import plugin_bridge as pb
    from app.preview import crop_to_transcript

    pb._guardar_faixa("doca", None, 20, [{"id": "p", "title": "p", "placement": "dock", "columns": 30, "tree": None}])
    try:
        pane = "\n".join(["ok" + " " * 18 + "│ Review !577", " " * 20 + "│"])
        assert crop_to_transcript("doca", pane) == "ok\n"
    finally:
        pb.esquecer("doca")

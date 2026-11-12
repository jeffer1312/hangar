import json
import logging
import os
import re
from typing import Optional

from app.models import StateEvent

_log = logging.getLogger("hangar.state")

SPINNER_GLYPHS = "✻✽✶✺✢·∗✳✦✧"
_OPTION_RE = re.compile(r"^\s*[❯>]?\s*\d+\.\s+(.*\S)\s*$")
# Bordas do box de `preview` do AskUserQuestion: renderiza NA MESMA LINHA da opção -> o label corta
# na primeira borda. Cobre os cantos ARREDONDADOS (╭╮╰╯) e os RETOS (┌┐└┘├┤┬┴┼) mais │ e ─: só os
# arredondados estavam aqui, e o box do preview desenha com os RETOS. Efeito medido: a opção que cai
# na linha da borda de cima vinha "Escolher dimensão +          ┌────────────" e nunca casava com o
# label do sidecar ("Escolher dimensão + valores"), então o gate do sse degradava pro OptionButtons —
# a pergunta perdia descrição E preview no app, e essa opção aparecia VAZIA. Toda pergunta com
# preview caía nisso, que é justamente a que mais precisa do stepper.
# `\s{2,}` antes da borda: o box do preview fica numa COLUNA à direita, então há sempre um vão de
# espaços entre o fim do label e a borda. Sem essa exigência, `─` (que é caractere de box, não o
# travessão —) cortava um label que o usasse no próprio texto: "Rodar tudo ─ inclusive os lentos"
# virava "Rodar tudo" e aí o casamento por prefixo aprovava um label ERRADO. `│` e os cantos seguem
# cortando sozinhos também: o box de `preview` de UMA coluna encosta no label sem o vão.
# `\s{3,}`: a coluna da direita do AskUserQuestion nem sempre tem moldura (o diff da última edição
# aparece solto ao lado da opção). Sem esse corte o label vinha "Postgres      +t como a", nenhuma
# pergunta com preview casava e o app caía no seletor do terminal.
_BOX_SPLIT_RE = re.compile(r"\s{2,}[│─╭╮╰╯┌┐└┘├┤┬┴┼]|[│╭╮╰╯┌┐└┘├┤┬┴┼]|\s{3,}")
_CURSOR_RE = re.compile(r"^\s*❯\s*\d+\.\s", re.M)
_SO_MOLDURA_RE = re.compile(r"^[\s│─╭╮╰╯┌┐└┘├┤┬┴┼]*$")


def _option(line: str) -> Optional[re.Match]:
    return _OPTION_RE.match(line)


# SEGUNDO marcador "N. " na mesma linha do cursor: a marca de uma lista digitada CORRIDA (rascunho no
# composer), porque no menu cada opcao ocupa a propria linha. Procurado SO no que vem depois do
# primeiro marcador (ver o uso) — aplicado na linha inteira ele casaria o proprio cursor, que e
# exatamente "❯ 1. " e mataria todo menu de verdade. Exige espaco antes e depois do numero de
# proposito: sem isso "versao 2.1" ou "R$ 2.50" dentro de uma opcao legitima matariam o menu, e
# recusar aqui significa uma sessao que pede resposta e nao avisa.
_MARCADOR_EMBUTIDO_RE = re.compile(r"\s\d+\.\s")
_RULE_RE = re.compile(r"^[\s─]*─{10,}[\s─]*$")  # a horizontal rule (the input box border)
_BOX_BOTTOM_RE = re.compile(r"^\s*╰[─\s]*╯\s*$")


def status_line(pane_text: str) -> Optional[str]:
    """The raw bottom chrome — the user's custom statusline + the mode line — returned
    verbatim so the web shows exactly what the terminal shows (each user has their own).

    It lives below the input box, i.e. after the last horizontal rule (Claude) or the last
    rounded composer box (Pi). No chrome below it -> None: an agent without a statusline
    extension shows NOTHING, never a slice of the conversation.
    """
    lines = pane_text.splitlines()
    last_rule = -1
    for i, ln in enumerate(lines):
        if _RULE_RE.match(ln) or _BOX_BOTTOM_RE.match(ln):
            last_rule = i
    if last_rule >= 0:
        chrome = [ln.rstrip() for ln in lines[last_rule + 1:] if ln.strip()]
    else:
        chrome = [ln.rstrip() for ln in lines if ln.strip()][-2:]
    return "\n".join(chrome) if chrome else None


def _question(lines: list[str]) -> Optional[str]:
    """The prompt text just above the first option in the menu region: the last meaningful
    line (skip rules, the ☐/☑ header chip, and blanks). Recebe ja a REGIAO do menu (nao o
    pane inteiro) pra nao pescar uma pergunta perdida no scrollback."""
    found = None
    for line in lines:
        if _option(line):
            break
        s = line.strip().strip("│").strip()
        if not s or _RULE_RE.match(line) or _SO_MOLDURA_RE.match(line) or s.lstrip("←").lstrip()[:1] in "☐☑":
            continue
        found = s
    return found


# Rodape de navegacao da AskUserQuestion (e de pickers similares). Ancora o limite INFERIOR
# do bloco. O menu nativo de permissao NAO tem esse rodape -> o limite cai num boundary.
_FOOTER_RE = re.compile(r"to navigate|Esc to cancel|Enter to select|Enter select")


def _rodape(lines: list[str], n: int = 8) -> str:
    """As `n` ultimas linhas do pane, DESCARTANDO as em branco do fim.

    O `capture-pane` devolve a altura inteira, entao um desenho no ALTO do pane (dialogo, picker
    curto) deixa o fundo vazio e empurra o rodape pra fora da janela — foi assim que o "Accessing
    workspace" do Claude Code passou por "tela livre" e o Enter do envio caiu no "No, exit" dele,
    matando a sessao. Mesma correcao que o `_pane_tail` do terminal_input ja fazia."""
    fim = len(lines)
    while fim and not lines[fim - 1].strip():
        fim -= 1
    return "\n".join(lines[max(0, fim - n):fim])


def is_overlay(pane_text: str) -> bool:
    # Overlay so-TUI aberto: rodape de navegacao por teclas no FUNDO do pane (ultimas 8 linhas — nao o
    # pane todo, senao a MESMA frase citada na conversa/scrollback dava falso-positivo). Cobre pickers
    # (/model) e paineis (/status, /config, /help) alem do AskUserQuestion. Fonte unica de "overlay"
    # (terminal_input.deliverable usa esta).
    return bool(_FOOTER_RE.search(_rodape(pane_text.splitlines())))


# Marcadores da tela de welcome/login do Claude Code (tema -> metodo -> URL OAuth -> colar code).
# Nenhum aparece numa sessao ja logada e em uso, entao servem de sinal de "precisa logar". ponytail:
# strings best-effort — se uma versao futura do claude mudar o texto, ajustar AQUI (calibration knob).
# Banner de limite de uso (feature #8), calibrado contra o Claude Code real (fixture
# pane_limite_uso.txt): o rodape mostra `⚠ Usage limit reached · continuing automatically at 9:10pm`
# e a conversa ecoa `You've hit your session limit · resets 9:10pm`. Frase e horario na MESMA linha.
_LIMIT_RE = re.compile(
    r"(?:usage limit reached|hit your \w+ limit|limit reached)"
    r"[^\n]{0,80}?"
    r"(?:resets?|continuing automatically|try again)\s*(?:at\s*)?"
    r"([0-9]{1,2}(?::[0-9]{2})?\s*(?:am|pm)?)",
    re.I,
)
# So o rodape, como o is_overlay: o banner mora la enquanto a sessao espera, e uma sessao que CITA
# o texto na conversa (saida de ferramenta, grep) ganhava o chip de limite sem ter limite nenhum.
_LIMIT_TAIL = 8


def rate_limit_reset(pane_text: str) -> Optional[str]:
    """Horario de volta do limite de uso (string crua, ex: "9:10pm"/"15:30") se o rodape do pane
    mostra o banner; None numa sessao normal."""
    m = _LIMIT_RE.search("\n".join(pane_text.splitlines()[-_LIMIT_TAIL:]))
    return m.group(1).strip() if m else None
# Glifos que marcam a BORDA do box do picker: bullet de assistente, junta de tool-result e
# spinners. Scrollback (incl. listas numeradas perdidas) vive alem dessas linhas.
_BOUNDARY_GLYPHS = "●⎿" + SPINNER_GLYPHS


def _is_boundary(line: str) -> bool:
    s = line.lstrip()
    return bool(s) and s[0] in _BOUNDARY_GLYPHS


def _menu_block(lines: list[str]) -> Optional[tuple[int, int]]:
    """Bounds [top, bot) do menu de selecao contiguo que contem o cursor ❯, ou None.

    Escopar a este bloco e o que mantem linhas numeradas do SCROLLBACK FORA das opcoes: subindo
    paramos no primeiro boundary (bullet/spinner); descendo paramos no rodape de navegacao ou
    no proximo boundary. Sem cursor ❯ N. nao ha menu (uma lista numerada solta nao e widget)."""
    cursor = None
    for i, ln in enumerate(lines):
        if _CURSOR_RE.match(ln):
            cursor = i   # cursor mais ao fundo = o picker vivo
    if cursor is None:
        return None
    if any(ln.lstrip()[:1] == "❯" and not _CURSOR_RE.match(ln)
           for ln in lines[cursor + 1:]):
        return None
    marca = _CURSOR_RE.match(lines[cursor])
    if marca:
        label = _BOX_SPLIT_RE.split(lines[cursor][marca.end():])[0]
        if _MARCADOR_EMBUTIDO_RE.search(label):
            return None
    # Rascunho numerado em várias linhas: o `❯ N.` é a própria linha de prompt, colada na régua de
    # cima do composer. Opção de menu de verdade nunca encosta na régua: vem depois do título ou da
    # pergunta. Rascunho não tem rodapé de navegação; com rodapé, segue como menu.
    if (cursor > 0 and _RULE_RE.match(lines[cursor - 1])
            and not any(_FOOTER_RE.search(ln) for ln in lines[cursor + 1:])):
        return None
    # Subindo do cursor, o topo do picker e o CHIP header (☐/☑ da AskUserQuestion) ou um boundary
    # (bullet/spinner do menu nativo, que nao tem chip). Parar no chip mantem FORA do bloco uma
    # LISTA NUMERADA EM PROSA da mensagem do assistente — ela vive acima do chip e, sem essa
    # ancora, "1. ... 2. ..." da prosa entrariam como opcoes falsas. A pergunta fica entre o chip
    # e as opcoes (com linha em branco no meio), entao continua dentro da regiao.
    # Na multipla escolha o chip vem depois de uma seta: "←  ☐ ... ✔ Submit  →".
    top = 0
    for i in range(cursor - 1, -1, -1):
        s = lines[i].lstrip()
        s = s[1:].lstrip() if s.startswith("←") else s
        if _is_boundary(lines[i]) or (bool(s) and s[0] in "☐☑"):
            top = i + 1
            break
    bot = len(lines)
    for i in range(cursor + 1, len(lines)):
        if _FOOTER_RE.search(lines[i]) or _is_boundary(lines[i]):
            bot = i
            break
    # Menu de verdade some com o composer: rodape com regua embaixo e um menu desenhado no TEXTO do
    # composer (recado colado), e tocar nele mandaria teclas para o prompt.
    if (bot < len(lines) and _FOOTER_RE.search(lines[bot])
            and any(_RULE_RE.match(ln) for ln in lines[bot + 1:]) and _prompt_acima(lines, cursor)):
        return None
    return (top, bot)


def _prompt_acima(lines: list[str], cursor: int) -> bool:
    """Logo abaixo da régua acima do cursor está a linha de prompt do composer (❯ sem número de opção)."""
    for i in range(cursor - 1, -1, -1):
        if _RULE_RE.match(lines[i]):
            nxt = lines[i + 1] if i + 1 < len(lines) else ""
            return nxt.lstrip().startswith("❯") and not _CURSOR_RE.match(nxt)
    return False


def _live_spinner(pane_text: str) -> Optional[str]:
    """The bottom-most spinner-glyph line (raw, incl. glyph), or None.

    The live status line sits at the bottom, just above the input box. Completed-turn
    markers ("✻ Worked for 8s") linger ABOVE it in the scrollback and look identical to a
    live spinner — so we take the bottom-most candidate; telling live from frozen needs
    two frames.
    """
    for line in reversed(pane_text.splitlines()):
        s = line.strip()
        if len(s) >= 2 and s[0] in SPINNER_GLYPHS and s[1] == " ":
            return s
    return None


# Seletor da TUI do Codex ANTES da thread (aprovar hooks, escolher login). Cursor proprio: `›`
# (U+203A), que o `classify` nao conhece — e nao entra la de proposito, porque o `classify` roda em
# TODO pane e um chevron solto no scrollback viraria menu fantasma. Aqui o portao e o rodape do
# proprio widget, que so existe com ele na tela.
_CODEX_OPT_RE = re.compile(r"^\s*[›>]?\s*(\d+)\.\s+(.*\S)\s*$")
# "confirm" nos seletores de hooks/permissoes; "continue" no aviso de atualizacao do CLI. A TUI
# mais nova encurtou o rodape para "enter continue · esc skip": sem a forma curta, o aviso nao vira
# cartao e a abertura parece travada.
_CODEX_RODAPES = ("enter to confirm", "enter to continue", "enter confirm", "enter continue")


def menu_codex(pane_text: str) -> Optional[tuple[Optional[str], list[str]]]:
    """(pergunta, opcoes) do seletor do Codex, ou None quando nao ha seletor na tela.

    Existe separado do `classify` porque a TUI do Codex nao tem regua nem composer pra ancorar a
    leitura — sem menu, ler o pane dela devolve as duas ultimas linhas como se fossem estado. Aqui
    so o widget e reconhecido: rodape presente, opcoes numeradas em sequencia (1, 2, 3...) e
    contiguas. Qualquer desvio disso devolve None, que e "nao sei", nao "nao ha".
    """
    linhas = pane_text.splitlines()
    # `_rodape` e nao `linhas[-6:]`: o widget desenha no ALTO do pane e o `capture-pane` devolve a
    # altura inteira, entao as ultimas linhas sao brancas — o mesmo tropeco do dialogo de confianca.
    rodape = _rodape(linhas).lower()
    if not any(r in rodape for r in _CODEX_RODAPES):
        return None
    opcoes: list[str] = []
    primeira = None
    coluna = 0
    for i, ln in enumerate(linhas):
        mm = _CODEX_OPT_RE.match(ln)
        if not mm:
            if not opcoes:
                continue
            # Opcao longa quebra em varias linhas num pane estreito: a continuacao vem sem numero
            # e NA MESMA COLUNA em que o texto da opcao comeca. Linha mais funda e descricao
            # (o picker de /permissions desenha `N. Rotulo   Descricao` em duas colunas e quebra
            # a descricao alinhada a ela) — essa nao e opcao, e o bloco fecha como antes.
            texto = ln.strip()
            if texto and len(ln) - len(ln.lstrip()) == coluna \
                    and not any(r in texto.lower() for r in _CODEX_RODAPES):
                opcoes[-1] = f"{opcoes[-1]} {texto}"
                continue
            break          # bloco contiguo: a primeira linha fora do padrao fecha o menu
        coluna = mm.start(2)
        if int(mm.group(1)) != len(opcoes) + 1:
            return None        # numeracao fora de ordem: nao e o widget
        if primeira is None:
            primeira = i
        opcoes.append(mm.group(2))
    if len(opcoes) < 2:
        return None
    # Pergunta = a ultima linha nao vazia acima do bloco (o titulo do widget).
    pergunta = next((ln.strip() for ln in reversed(linhas[:primeira]) if ln.strip()), None)
    return pergunta, opcoes


# Diálogo do Claude Code desenhado com `hideIndexes` (confiança da pasta, "Make auto mode your
# default permission mode?"): as opções vêm sem número e o `_CURSOR_RE` não o enxerga. Sem isto a
# sessão parecia ociosa e o envio digitava em cima da pergunta.
_CURSOR_SEM_NUMERO_RE = re.compile(r"^(\s*❯\s+)\S")
# Dicas que o Claude Code põe no rodapé quando o foco está nele, no lugar de "↓ to manage".
_FOCO_NO_RODAPE_RE = re.compile(r"Enter to view|↑/↓ to select")
# O cursor do painel de agentes: só na frente de um agente, para uma statusline com `❯` não contar.
_CURSOR_DO_PAINEL_RE = re.compile(r"^\s*❯\s+[●◯]")


def _fim_do_composer(lines: list[str], ate: Optional[int] = None) -> Optional[int]:
    """A régua de baixo da última caixa do composer (régua, `❯`, régua) antes de `ate`, ou None.
    Abaixo dela mora o rodapé do Claude Code."""
    regras = [j for j, ln in enumerate(lines[:ate]) if _RULE_RE.match(ln)]
    if len(regras) < 2 or not any(ln.lstrip().startswith("❯") for ln in lines[regras[-2] + 1:regras[-1]]):
        return None
    return regras[-1]


def _painel_de_agentes(lines: list[str], top: int, options: list[str]) -> bool:
    # O painel de agentes ("← for agents": ● principal, ◯ subagente) com foco põe o `❯` num agente e
    # parece um menu sem número; responder ao cartão dele navegava no painel.
    return all(o[:1] and o[0] in "●◯" for o in options) and _fim_do_composer(lines, top) is not None


def foco_no_rodape(pane_text: str) -> bool:
    """O teclado está no rodapé do Claude Code (painel de agentes ou pílula de tarefas), não no
    composer: o que se digita some, e o `x` do painel para um subagente."""
    lines = pane_text.splitlines()
    fim = _fim_do_composer(lines)
    return fim is not None and any(_CURSOR_DO_PAINEL_RE.match(ln) or _FOCO_NO_RODAPE_RE.search(ln)
                                   for ln in lines[fim + 1:])


def _menu_sem_numero(lines: list[str]) -> Optional[tuple[Optional[str], list[str], int]]:
    """(pergunta, opcoes, posicao do cursor 1-based) do diálogo sem numeração, ou None.

    Só é menu vivo sem régua ABAIXO do cursor: o `❯ ` do composer mora sempre entre duas réguas,
    e uma citação no scrollback tem o composer embaixo dela. As opções são as linhas contíguas
    que começam na coluna do rótulo do cursor."""
    cursor = None
    for i, ln in enumerate(lines):
        if _CURSOR_SEM_NUMERO_RE.match(ln) and not _CURSOR_RE.match(ln):
            cursor = i
    if cursor is None or any(_RULE_RE.match(ln) for ln in lines[cursor + 1:]):
        return None
    col = _CURSOR_SEM_NUMERO_RE.match(lines[cursor]).end(1)

    def na_coluna(ln: str) -> bool:
        return len(ln) > col and not ln[:col].strip() and ln[col] != " "

    top = cursor
    while top > 0 and na_coluna(lines[top - 1]):
        top -= 1
    bot = cursor + 1
    while bot < len(lines) and na_coluna(lines[bot]):
        bot += 1
    options = [lines[i][col:].strip() for i in range(top, bot)]
    if len(options) < 2 or _painel_de_agentes(lines, top, options):
        return None
    # A pergunta é o título: primeira linha com texto depois da régua que abre o diálogo.
    inicio = max((i for i in range(top) if _RULE_RE.match(lines[i])), default=-1) + 1
    question = next((ln.strip() for ln in lines[inicio:top] if ln.strip()), None)
    return question, options, cursor - top + 1


def cursor_sem_numero(pane_text: str) -> Optional[int]:
    """Opção (1-based) sob o cursor do diálogo sem numeração, ou None sem diálogo na tela."""
    menu = _menu_sem_numero(pane_text.splitlines())
    return menu[2] if menu else None


def classify(pane_text: str) -> tuple[str, Optional[str], Optional[str], Optional[list[str]]]:
    """Return (state, label, question, options).

    'working' -> label is the live spinner text; 'awaiting_input' -> question +
    options; otherwise 'idle'. Never 'dead': that is the caller's call.

    NOTE: a single static pane cannot tell a live spinner from a frozen completed-turn
    marker (both render as "<glyph> <word> for <N>s"). classify reports 'working' for any
    spinner candidate; a caller that needs the difference compares two frames.
    """
    lines = pane_text.splitlines()
    block = _menu_block(lines)
    if block is not None:
        top, bot = block
        region = lines[top:bot]
        # AskUserQuestion com `preview` renderiza um box (│...│) NA MESMA LINHA da opção — sem o
        # corte, o conteúdo do preview entrava no label ("Alfabético (obedece │ using System..."). A
        # label ainda pode vir truncada por wrap de coluna; o gate do stepper (sse) casa por prefixo.
        options = [_BOX_SPLIT_RE.split(m.group(1))[0].strip()
                   for m in (_option(ln) for ln in region) if m]
        options = [o for o in options if o]
        # DUAS opcoes, no minimo: escolher entre uma coisa so nao e escolha, e todo menu real medido
        # tem de 3 a 5 (as cinco fixtures de pane deste repo). Uma sozinha e sinal de que o bloco foi
        # montado em cima de texto que apenas PARECE lista — o rascunho no composer e o caso vivo.
        if len(options) >= 2:
            return ("awaiting_input", None, _question(region), options)
    else:
        sem_numero = _menu_sem_numero(lines)
        if sem_numero is not None:
            return ("awaiting_input", None, sem_numero[0], sem_numero[1])

    spinner = _live_spinner(pane_text)
    if spinner is not None:
        return ("working", spinner[2:].strip(), None, None)

    return ("idle", None, None, None)


_KIMI_CHUNK = 64 << 10


# Uma linha por arquivo, nao por chamada: quem chama e o poll da lista (~5s por sessao), entao um
# wire ilegivel viraria uma linha de log a cada ciclo, pra sempre. O sinal aqui e "aconteceu", nao a
# frequencia. Sem expiracao de proposito — o conjunto tem no maximo uma entrada por sessao viva.
_KIMI_AVISADOS: set[str] = set()


def _avisa_uma_vez(chave: str, msg: str, *args) -> None:
    if chave in _KIMI_AVISADOS:
        return
    _KIMI_AVISADOS.add(chave)
    _log.warning(msg, *args)


def _turno_aberto(jsonl: str, filtro: re.Pattern, decide, teto: int, aviso: str) -> Optional[bool]:
    """Le o fim do arquivo de TRAS PRA FRENTE ate a primeira linha que `decide` reconheca como
    fronteira de turno. True = turno aberto, False = fechado, None = nao deu pra saber.

    Na pratica isso e uma ou duas linhas, porque o transcript de uma sessao parada termina na
    propria fronteira. O `teto` existe pro caso patologico (arquivo sem evento de turno nenhum):
    melhor devolver None do que varrer megabytes a cada poll.

    `filtro` e so o filtro BARATO (regex sobre os bytes) — quem decide e o `decide`, que le o json.
    Sem essa separacao, uma mensagem do usuario CITANDO o nome do evento vira fronteira.
    """
    try:
        with open(jsonl, "rb") as fh:
            fh.seek(0, os.SEEK_END)
            fim = pos = fh.tell()
            resto = b""
            while pos > 0 and fim - pos < teto:
                passo = min(_KIMI_CHUNK, pos)
                pos -= passo
                fh.seek(pos)
                linhas = (fh.read(passo) + resto).split(b"\n")
                # linhas[0] so esta INTEIRA quando ja chegamos ao inicio do arquivo; ate la ela e o
                # rabo cortado pelo chunk e volta colada no proximo pedaco.
                resto, inicio = (b"", 0) if pos == 0 else (linhas[0], 1)
                for ln in reversed(linhas[inicio:]):
                    if not filtro.search(ln):
                        continue
                    try:
                        obj = json.loads(ln)
                    except (ValueError, AttributeError):
                        continue
                    r = decide(obj)
                    if r is not None:
                        return r
    except OSError:
        # None faz o chamador voltar a decidir SO pelo mtime — que e exatamente o comportamento que
        # esta funcao existe pra corrigir. Sem log, "decidiu False" e "desistiu e caiu no mtime"
        # ficam indistinguiveis, e uma sessao presa em "em execucao" nao tem onde ser diagnosticada.
        _avisa_uma_vez(jsonl, aviso + ": nao deu pra ler o fim de %s", jsonl)
        return None
    _avisa_uma_vez(jsonl, aviso + ": nenhuma fronteira de turno no fim de %s", jsonl)
    return None


# Fronteira de turno no rollout do Codex. `task_started` abre; `task_complete` e `turn_aborted`
# fecham (medido nos rollouts reais: um turno interrompido termina em `turn_aborted`, sem
# `task_complete`). Sao `event_msg`, entao o tipo mora no `payload`, nao no topo da linha.
_CODEX_ABRE = ("task_started",)
_CODEX_FECHA = ("task_complete", "turn_aborted")
_CODEX_TURNO_RE = re.compile(rb'"type"\s*:\s*"(task_started|task_complete|turn_aborted)"')
_CODEX_TETO = 4 << 20


def codex_turno_aberto(jsonl: str, teto: int = _CODEX_TETO) -> Optional[bool]:
    """Ha turno ABERTO no fim do rollout do Codex? True/False; None = nao deu pra saber.

    Quem pergunta e a lista: sessao Codex com turno andando e marcador de estado NENHUM significa
    que o hook nao esta rodando — e a unica causa conhecida disso e hook nao aprovado na TUI."""
    def decide(obj):
        if obj.get("type") != "event_msg":
            return None
        tipo = (obj.get("payload") or {}).get("type")
        return False if tipo in _CODEX_FECHA else (True if tipo in _CODEX_ABRE else None)

    return _turno_aberto(jsonl, _CODEX_TURNO_RE, decide, teto, "codex")

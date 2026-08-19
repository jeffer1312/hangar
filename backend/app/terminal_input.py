import logging
import os
import re
import threading
import time

from app import agentpane
from app import kimi_models
from app import model_picker as mp
from app import pi_inbox
from app import tmux
from app.models import scrub_surrogates
from app.pqueue import PromptQueue, _transcript_start_ts
from app.state import _live_spinner, classify, is_overlay
from app.tmux import send_keys

_log = logging.getLogger("claude_pocket.terminal_input")

# Tempos de acomodacao do TUI entre toque e leitura do pane (o picker redesenha em overlay).
_SETTLE = 0.3  # apos uma tecla de navegacao
_OPEN_SETTLE = 0.7  # apos abrir o picker / confirmar (precisa redesenhar/commitar o resultado)
_NAV_GAP = 0.12  # entre toques Up/Down em rajada
# Id de modelo aceito na troca: alfanumérico, opcionalmente com o sufixo de janela do próprio
# Claude Code (`opus[1m]`). É o que separa as duas linhas `opus` do picker. Este valor NÃO vira
# tecla — ele só resolve qual linha navegar, e o que sai pro tty é Up/Down/s/Enter. Quem digita id
# literal é o `set_engine_model`, com gate próprio.
_MODEL_KW_OK = re.compile(r"^[a-z0-9]+(\[[a-z0-9]+\])?\Z")
_SLASH_SETTLE = 0.3  # apos digitar "/cmd": deixa o menu de autocomplete renderizar antes do Enter
_SUBMIT_SETTLE = 0.2  # entre o texto livre e o Enter: claude detecta input rapido como paste e engole o Enter
# Idem pro MULTILINHA, que estava em 0.05 - MENOR que o do caminho de uma linha, embora seja o
# caminho LENTO (paste_text faz 2N-1 chamadas de tmux no Windows, nao 2). Resultado: o Enter
# corria a ingestao e submetia so o comeco; foi assim que um recado de 2 KB entre sessoes chegou
# cortado na primeira linha. MEDIDO na TUI real (claude v2.1.218 + psmux 3.3.7), tempo entre o
# paste_text retornar e o fim do texto aparecer no pane:
#     575 chars/6 linhas 0.13s | 1151/12 0.29s | 2303/24 0.16s | 3839/40 0.08s
# Nao escala com o tamanho (o envio ja e espacado, a TUI acompanha) - o pico medido e 0.29s, e o
# MINIMO ja era 0.08s, ou seja 0.05 perdia em 4/4 dos casos. 0.5 da ~1.7x de folga sobre o pico,
# e e ruido perto dos 0.4-2.9s que o proprio envio custa.
# CUIDADO ao re-medir: a caixa de input ROLA. Com 24 linhas as 4 primeiras nao aparecem no
# capture-pane, o que parece perda de dados e nao e - confira pelo fim do texto, nunca pelo comeco.
# ponytail: constante, nao malha fechada. Se escapar em maquina lenta, o upgrade e esperar o ECO
# do fim do texto no pane antes do Enter (mesma ideia ja anotada no ramo de uma linha).
_MULTILINE_SUBMIT_SETTLE = 0.5
# Conferencia de que o input limpou depois do Enter. INSISTE com prazo em vez de tirar UMA foto: o
# argumento de "foto unica basta porque redraw incompleto mostra MENOS texto" tem furo — se a captura
# correr ANTES de qualquer redraw, a tela e a VELHA, com o texto inteiro no composer, indistinguivel
# de "nao submeteu". Isso seria falso positivo, o pior erro possivel aqui: o remetente ve erro,
# reenvia, e o reenvio digita em cima do residuo.
# Insistindo, o caminho que DEU CERTO sai na primeira leitura limpa (~0.15s) e so quem realmente nao
# submeteu paga o orcamento inteiro antes de ser acusado.
# O teto de 1.0s tem folga de ~3x sobre o pico medido de ingestao multi-linha (0.29s, os numeros estao
# em _MULTILINE_SUBMIT_SETTLE acima). Nao e medicao do redraw pos-Enter — essa falta, e o par no
# Windows vai medir; ate lá o prazo cobre o pior caso conhecido do vizinho.
# ponytail: constante com folga, nao malha fechada. Se aparecer maquina lenta o suficiente pra estourar
# 1.0s, o upgrade e ler o transcript (fonte de verdade) em vez da tela — o que o _confirm_and_drain
# ja faz 8s depois; esta checagem existe pra ANTECIPAR o sinal, nao pra substituir aquele.
_SUBMIT_CHECK_INTERVALO = 0.15
_SUBMIT_CHECK_PRAZO = 1.0
# Prazo da prova da colagem por clipboard (Windows). NAO herda o _SUBMIT_CHECK_PRAZO (1.0s): aquele
# saiu do paste-buffer do tmux no Linux, e o comentario dele avisava que a medicao no Windows
# faltava. Ela existe agora (docs/medicoes-2026-08-08-windows.md): 5 colagens de 600 linhas ate o
# `[Pasted text #N]` aparecer deram 676, 665, 955, 1341 e 975 ms — 1.0s falharia em 3 das 5, e um
# settle fixo de 0.5s falharia em TODAS. 4.0s = 3x o pico medido, o mesmo criterio de folga do
# vizinho.
# CUIDADO: o tempo CRESCE com o numero de colagens da mesma sessao (as duas primeiras na casa dos
# 670ms, as ultimas acima de 950). Se estourar numa sessao de vida longa, a saida NAO e mandar Enter
# assim mesmo — e falhar alto, que e o que _provou_entrega faz devolvendo False.
_PROVA_PRAZO = 4.0


def _entrou_no_composer(name: str, texto: str, pastes_antes: set[str] | None = None) -> bool:
    """True = a cauda OU o comeco do texto APARECEU no composer, ou seja o multiplexador entregou
    de fato.

    Evidencia POSITIVA antes do Enter. Sem ela, "composer vazio" e ambiguo: significa tanto "submeteu"
    quanto "nunca entrou nada" — e o segundo caso e real, medido no psmux, onde set-buffer e
    paste-buffer devolvem rc=0 sem entregar nada (ver tmux.buffer_trunca_no_newline). Era esse o furo
    do _submeteu sozinho: ele via composer vazio, concluia entrega, gravava delivered=True, e o
    reconcile depois redigitava o texto por nao achar no transcript — as rajadas de 3.

    Precisa do comeco ALEM da cauda (medido 02/08/2026, ver _RESIDUO_INICIO em _composer_residuo):
    com texto longo o composer do Claude Code so desenha as primeiras linhas, entao a cauda nunca
    aparece e este gate segurava o Enter mesmo com o texto genuinamente entregue.
    """
    fim = time.monotonic() + _SUBMIT_CHECK_PRAZO
    while True:
        r = _composer_residuo(_capture(name), texto, name, pastes_antes)
        if r is not False:
            # True = evidencia de que entrou. None = nao da pra provar (cauda curta, pane ilegivel) ->
            # SEGUE EM FRENTE. Bloquear no "nao sei" faria toda mensagem curta parar de ser enviada, e
            # e a mesma politica que o _wait_input_ready ja adota: na duvida, envia e avisa.
            return True
        if time.monotonic() >= fim:
            return False
        time.sleep(_SUBMIT_CHECK_INTERVALO)


def _provou_entrega(name: str, texto: str, pastes_antes: set[str] | None) -> bool:
    """Prova ESTRITA de que a colagem chegou: so `is True` conta.

    Diferente de `_entrou_no_composer`, que aceita o indefinido pra nao travar o caminho de sempre.
    Aqui o indefinido NAO pode liberar o Enter: no caminho do clipboard, "nao sei" pode significar
    que a tecla nao foi entendida e o composer esta VAZIO — e o Enter submeteria o nada, ou pior, o
    que ja estivesse la. Medido na winboat: a TUI descarta o Alt+V enquanto esta processando, e nesse
    caso nada aparece no composer e nenhum comando devolve erro.

    LIMITE ASSUMIDO, e ele importa: isto prova que ALGUMA COISA foi colada, nunca O QUE. Numa
    mensagem grande a TUI colapsa em `[Pasted text #N]` e o conteudo nunca e desenhado, entao nao ha
    como comparar. E por isso que o `tmux._CLIP_LOCK` e obrigatorio e fica segurado ate aqui: ele
    fecha a unica janela em que o conteudo colado poderia ser de outra mensagem.
    """
    prazo = time.monotonic() + _PROVA_PRAZO
    while True:
        if _composer_residuo(_capture(name), texto, name, pastes_antes) is True:
            return True
        if time.monotonic() >= prazo:
            return False
        time.sleep(_SUBMIT_CHECK_INTERVALO)


def _submeteu(name: str, texto: str, pastes_antes: set[str] | None = None) -> bool:
    """True = o composer limpou (submeteu). False = a cauda OU o comeco do texto continuam lá
    depois do prazo.

    So vale como prova DEPOIS do _entrou_no_composer: sozinho, composer vazio nao distingue submissao
    de nao-entrega.

    O comeco tambem conta aqui, e nao por acidente: a pergunta desta funcao e "o residuo sumiu?", e
    "o comeco sumiu" prova submissao tao bem quanto "a cauda sumiu" — o Enter limpa o composer
    inteiro, entao se um sumiu o outro tambem sumiu. Pro caso que motivou o comeco (texto longo, cauda
    nunca chega a ser desenhada), o comeco e ate a UNICA evidencia disponivel: sem ele, esta funcao
    nunca teria como saber "cauda sumiu" (ela nunca esteve visivel) e o texto ficaria preso perguntando
    pra sempre."""
    fim = time.monotonic() + _SUBMIT_CHECK_PRAZO
    while True:
        time.sleep(_SUBMIT_CHECK_INTERVALO)
        # `is not True`: False (limpou) e None (nao sei) valem como submetido — degrada pro
        # comportamento anterior a esta checagem existir, nunca inventa falha.
        if _composer_residuo(_capture(name), texto, name, pastes_antes) is not True:
            return True
        if time.monotonic() >= fim:
            return False


def _capture(name: str) -> str:
    """Lê o pane atual da sessão tmux (wrapper em módulo para permitir patch nos testes)."""
    return tmux.capture_pane(name)


# Marcas do rodapé do Claude Code quando o input já está VIVO (TUI interativo). Durante o BOOT
# (logo + carregamento) elas ainda não aparecem.
# O rodape ATUAL (medido no claude v2.1.218) e `⏵⏵ auto mode on (shift+tab to cycle) · ← for agents`,
# `⏸ manual mode on · ← for agents` ou `⏵⏵ bypass permissions on (...)` — nenhuma das marcas antigas
# aparece nos dois primeiros, entao TODO envio queimava os 12s de timeout com o _send_lock na mao
# (mesmo estrago que o `╰─` do Pi documentado abaixo).
# O marcador e o GLIFO de modo, nao a frase "mode on": `_capture` le a tela INTEIRA, e "mode on" e
# frase de duas palavras comuns — uma conversa citando "auto mode on", um `git show` deste commit ou
# prosa com "debug mode on" ja casava, e o gate liberava com a TUI ainda bootando (mensagem engolida,
# exatamente o bug que ele existe pra evitar). O proprio comentario anterior avisava dessa armadilha
# ("discutir os marcadores no chat faz eles casarem com o texto da conversa") e caiu nela. `⏵⏵`/`⏸`
# cobrem os MESMOS rodapes e nao aparecem em prosa.
# As marcas antigas ficam pra versoes/telas anteriores. Se um dia o glifo mudar, o gate estoura o
# timeout e AVISA uma vez (_warn_ready_timeout_once) em vez de ficar lento e calado.
_READY_MARKERS = ("⏵⏵", "⏸", "bypass permissions", "? for shortcuts", "for shortcuts")

# Quantas linhas do FIM do pane valem como "rodape" pra procurar marcador. O aviso acima ("cuidado ao
# medir, o _capture le a tela inteira") virou codigo: casar no pane TODO deixa o gate refem do texto
# da CONVERSA. "mode on" e frase de duas palavras comuns — um chat que cite `auto mode on`, um
# `git show` deste commit, ou prosa com "debug mode on" ja casa. E o caso quebrado nao e teorico: numa
# sessao reatachada/respawnada o pane ainda mostra a tela anterior por instantes, com a TUI ainda NAO
# aceitando teclas -> o gate libera cedo e a mensagem e engolida, que e exatamente o bug que ele
# existe pra evitar. O rodape do Claude Code mora sempre nas ultimas linhas (regua + 2-3 de statusline
# + a linha de modo), e o composer do Pi tambem — entao a cauda serve pros dois providers, e nos
# marcadores do Pi (glifos de moldura `─`/`│`) o ganho e ainda maior: qualquer tabela no meio da
# conversa casava.
# 12 linhas: a statusline do usuario ocupa 3 e o composer do Pi ~3, entao sobra folga de 2x.
# ponytail: janela fixa; upgrade = casar por ancora de rodape (regua + statusline) se algum provider
# passar a desenhar rodape alto.
_READY_TAIL_LINES = 12


def _pane_tail(pane: str, lines: int = _READY_TAIL_LINES) -> str:
    # rstrip("\n") primeiro: pane com linhas em branco no fim (comum apos redraw) empurraria o rodape
    # pra fora da janela e o gate voltaria a esperar os 12s inteiros.
    return "\n".join(pane.rstrip("\n").split("\n")[-lines:])

# Por PROVIDER. O Pi não imprime rodapé nenhum: a prova de "TUI viva" é o CHROME do composer —
# qualquer caractere de moldura na tela. NÃO é uma moldura específica: medido no pi 0.82.1, o mesmo
# composer é desenhado de três jeitos diferentes (caixa `╭──╮ … ╰──╯` em versões antigas; DUAS
# réguas `────` no pi puro `--no-extensions`; as mesmas réguas + statusline com o pacote
# `pi-claude-code-ui` que o usuário usa). Casar UM desenho (`╰─`) foi o bug: com a UI atual o pane
# não tem nenhum `╰─`, então TODO envio queimava os 12s de timeout com o _send_lock na mão.
# Por que qualquer moldura serve como prova: no interactive-mode.js do Pi o editor entra na tela
# (`addChild(editorContainer)` + `setFocus` + `setupKeyHandlers` + `ui.start()`) ANTES do header e
# do `renderInitialMessages()` — nada é desenhado antes de as teclas serem aceitas. E o boot (que
# baixa `fd`/`rg`) não imprime uma moldura sequer: medido em dois boots reais capturados a cada
# 0.1–0.25s, o primeiro caractere de moldura aparece em 4.2s / 4.5s, junto com o composer.
# ponytail: o conjunto de glifos é calibration knob, igual ao _LOGIN_RE do state.py.
_READY_MARKERS_BY_PROVIDER = {"pi": ("─", "━", "═", "╰", "│"),
                              # Kimi: composer = mesma caixa arredondada do Pi (╭─╮ ╰─╯ com "> "
                              # dentro) — medido num pane real do Kimi 0.34.0. O conjunto do Pi vale
                              # inteiro: qualquer moldura em tela prova que a TUI ja aceita tecla.
                              "kimi": ("─", "━", "═", "╰", "│")}

# Timeout por provider. O Pi ficou mais curto de propósito: o boot medido até o composer é ~4.3s,
# então 8s é ~2× de folga, e no estouro a gente ENVIA mesmo assim — ou seja, a espera só compra
# segurança durante o boot e todo o resto é latência pura no dia em que o marcador desandar de novo.
_TIMEOUTS_BY_PROVIDER = {"pi": 8.0, "kimi": 8.0}
_DEFAULT_TIMEOUT = 12.0

# Um aviso por (sessão, provider): marcador que para de casar não pode ser silencioso — foi assim
# que os 12s por mensagem chegaram em produção. Mesma forma do _warn_bilhete_once do registry.
_READY_TIMEOUT_WARNED: set[tuple[str, str]] = set()
# Idem pro composer ilegivel: checagem que morre calada e o mesmo estrago do marcador que para de casar.
_COMPOSER_WARNED: set[str] = set()
# Defer por composer do Pi ocupado: o drain roda em TODO reconnect/idle — com um rascunho parado no
# composer o mesmo WARNING repetiria pra sempre. Uma vez por sessao; sai do set quando desocupa,
# pra um NOVO episodio avisar de novo.
_OCUPADO_WARNED: set[str] = set()
_OCUPADO_DEFER_COUNT: dict[str, int] = {}
# Idem pro deferred de sessao INDISPONIVEL (`not deliverable`: overlay/menu aberto no terminal) —
# achado da review 02/08/2026: era o UNICO "deferred" do arquivo sem log nenhum. Um overlay preso
# vira todo envio em adiamento silencioso pra sempre, e o usuario so descobre olhando o terminal.
_INDISPONIVEL_WARNED: set[str] = set()
_INDISPONIVEL_DEFER_COUNT: dict[str, int] = {}
# Quantas vezes SEGUIDAS uma sessao pode ficar deferred (por composer ocupado OU por indisponivel)
# antes do log virar ERRO, em vez do WARNING unico de praxe (_OCUPADO_WARNED/_INDISPONIVEL_WARNED),
# que cala depois da primeira vez. Medido 02/08/2026: com o aviso de subagente do Pi contando como
# "ocupado" (bug hoje corrigido em _composer_ocupado_pi), o usuario so descobria a fila emperrada
# mexendo no terminal a mao — "adiar pra sempre e pior que avisar" vale tambem pro caso em que o
# composer FICA ocupado de verdade (um rascunho de longa duracao, por exemplo) ou a sessao fica presa
# num overlay. Contador por sessao; zera quando desocupa/fica disponivel de novo (ou quando a sessao
# MORRE — ver _limpa_deferred —, senao os dicts cresceriam pra sempre pra um nome que nunca mais volta).
_OCUPADO_DEFER_LIMIT = 5


def _avisa_deferred(name: str, motivo: str, avisados: set[str], contadores: dict[str, int],
                     diag: str) -> None:
    """WARNING uma vez (aviso-uma-vez de praxe, ver _COMPOSER_WARNED/_READY_TIMEOUT_WARNED);
    acima de _OCUPADO_DEFER_LIMIT tentativas seguidas, ERRO — mas com TREGUA (na virada do limite e
    depois a cada _OCUPADO_DEFER_LIMIT tentativas, nao em TODA tentativa): sem tregua, uma fila
    emperrada de verdade inunda o journal a cada drain/reconexao sem nunca entregar (achado da
    review 02/08/2026) — o resto do arquivo cala de vez apos o primeiro aviso, mas aqui a intencao e
    o oposto: o ERRO precisa voltar a aparecer de tempos em tempos, nao ficar mudo pra sempre."""
    n = contadores[name] = contadores.get(name, 0) + 1
    if n > _OCUPADO_DEFER_LIMIT:
        if n == _OCUPADO_DEFER_LIMIT + 1 or n % _OCUPADO_DEFER_LIMIT == 0:
            _log.error("send adiado ha %d tentativas seguidas name=%s: %s — fila pode estar "
                       "emperrada, confira o terminal — %s", n, name, motivo, diag)
    elif name not in avisados:
        avisados.add(name)
        _log.warning("send adiado name=%s: %s — deferred (aviso unico ate desocupar) — %s",
                     name, motivo, diag)


def _limpa_deferred(name: str, avisados: set[str], contadores: dict[str, int]) -> None:
    avisados.discard(name)
    contadores.pop(name, None)


def _warn_composer_ilegivel_once(name: str) -> None:
    if name and name not in _COMPOSER_WARNED:
        _COMPOSER_WARNED.add(name)
        _log.warning(
            "%s: nao achei a regiao do composer no pane (menos de 2 reguas) — a conferencia de "
            "'o Enter submeteu?' fica INERTE nesta sessao e o envio volta a afirmar entrega sem checar. "
            "Se o TUI mudou de desenho, medir e ajustar: ver _composer_residuo em terminal_input.py",
            name)


def _warn_ready_timeout_once(name: str, provider: str, timeout: float) -> None:
    if (name, provider) not in _READY_TIMEOUT_WARNED:
        _READY_TIMEOUT_WARNED.add((name, provider))
        _log.warning(
            "%s: pane de %s nao casou nenhuma marca de TUI pronta em %.1fs — enviando assim mesmo. "
            "Se a sessao esta VIVA e respondendo, o marcador desandou (o TUI mudou de desenho) e "
            "cada mensagem paga esse tempo: ver _READY_MARKERS_BY_PROVIDER em terminal_input.py",
            name, provider, timeout)


# Quanto do FIM do texto enviado a gente procura no composer pra decidir "nao submeteu". No caso
# ORIGINAL (submissao truncada a meio) sobra justamente o FIM, entao a cauda e a prova certa ali.
# (Ate 02/08/2026 este comentario dizia "e nao o comeco, que aparece no ECO da conversa acima do
# composer" como razao pra NUNCA olhar o comeco. Isso e verdade so se a comparacao rodar contra o
# PANE INTEIRO; _composer_residuo, logo abaixo, sempre restringiu a busca a REGIAO do composer —
# entre as duas ultimas reguas, ver _composer_regiao — e o eco fica ACIMA da primeira regua. Nessa
# regiao o comeco e prova tao segura quanto a cauda, entao virou evidencia valida — ver
# _RESIDUO_INICIO logo abaixo.)
_RESIDUO_CAUDA = 40
# Quanto do COMECO do texto enviado a gente procura, como evidencia ALTERNATIVA a cauda. Causa raiz
# medida 02/08/2026 (log real, duas ocorrencias seguidas): o composer do Claude Code CORTA a EXIBICAO
# de texto longo — desenha as primeiras linhas e o resto (onde mora a cauda) nunca aparece. Pior com
# imagem anexada, porque o caminho do arquivo colado vai pro FIM do texto e vira a cauda:
#   cauda='ude-pocket-uploads/1785666473-67f17f.png'
#   composer='───\n❯ [Image #1]Então aconteceu várias outras vezes aqui, mas imagino que seu oque é ,
#   todas no pi ,\n  As vezes dps de rodar um subagnt ele aparece essa sugestão, não tem nd digitado
#   aí , seu eu for\n  pelo terminal e escrever vai , mas se tá usando a visualização no pane talvez
#   não quer enviar,\n  vi'
# O texto ESTAVA la (da pra ver o comeco) — so a cauda que nunca foi desenhada, entao
# _entrou_no_composer via False, o Enter nunca ia, e cada retry do app empilhava OUTRO envio (mensagem
# triplicada que o usuario relatou). O comeco e exatamente a parte que o composer desenha.
_RESIDUO_INICIO = 40
# Minimo de caracteres (sem espaco) pra a cauda OU o comeco valerem como prova. Ver _composer_residuo.
_RESIDUO_MIN = 12
# A regua de BAIXO do composer tem de estar nas ultimas N linhas da tela, e as duas reguas a no maximo
# M linhas uma da outra. Sem isso o par pode cair na divisoria do banner + regua de cima do composer.
_COMPOSER_FUNDO = 8
_COMPOSER_ALTURA = 15


def _sem_espaco(s: str) -> str:
    return re.sub(r"\s+", "", s)


# Numeros dos placeholders de paste numa regiao do pane. A IDENTIDADE importa: aceitar qualquer
# placeholder fazia um paste ALHEIO (rascunho do usuario) contar como a nossa entrega — o Enter
# submetia o texto do usuario como se fosse o prompt do agente (achado CRITICO da review de 31/07).
# So um numero NOVO em relacao a foto tirada ANTES do nosso paste prova alguma coisa.
#
# DOIS desenhos, um por agente, e cada um tem de estar aqui porque o texto real nunca chega a ser
# desenhado quando o TUI colapsa:
#   Claude Code -> "[Pasted text #1 +12 lines]"
#   Pi          -> "[paste #1 1032 chars]"      (medido 09/08/2026, pi 0.83.0)
# So o do Claude estava coberto, e o do Pi custou o bug que o usuario relatou: recado longo de
# pareamento entrava no composer do Pi, `_composer_residuo` nao achava prova nenhuma, o Enter nunca
# era enviado e cada retry empilhava outro paste (o log do proprio codigo imprimia
# `composer='… [paste #1 1032 chars] …' pastes(antes=[] depois=[])` — mostrando o chip na tela e o
# detector cego). Depois do primeiro travamento o `_composer_ocupado_pi` passava a adiar tudo, e a
# fila so drenava com um Enter manual no terminal, entregando a pilha de uma vez.
# ponytail: e uma lista de desenhos medidos, igual ao _READY_MARKERS_BY_PROVIDER — provider novo que
# colapse paste entra aqui, com a forma medida no pane, nunca chutada.
_PASTE_ID_RE = re.compile(r"\[(?:Pasted text|paste) #(\d+)")


def _paste_ids(regiao: str) -> set[str]:
    return set(_PASTE_ID_RE.findall(regiao))


def _composer_regiao(pane: str, nome_sessao: str = "") -> str | None:
    """Regiao do composer (entre as duas ultimas reguas) ou None se ilegivel (avisa uma vez)."""
    linhas = pane.split("\n")
    reguas = [i for i, l in enumerate(linhas) if l.count("─") >= 20]
    if len(reguas) >= 2 and (
            len(linhas) - reguas[-1] > _COMPOSER_FUNDO or reguas[-1] - reguas[-2] > _COMPOSER_ALTURA):
        reguas = []
    if len(reguas) < 2:
        _warn_composer_ilegivel_once(nome_sessao)
        return None
    return "\n".join(linhas[reguas[-2]:reguas[-1] + 1])


def _composer_residuo(pane: str, texto: str, nome_sessao: str = "",
                      pastes_antes: set[str] | None = None) -> bool | None:
    """True = a cauda OU o comeco do texto estao no composer. False = NENHUM esta. None = NAO DA
    PRA SABER.

    Tres estados e nao dois: o mesmo "nao sei" precisa cair pra lados OPOSTOS nos dois chamadores.
    Pro _submeteu (residuo sumiu? entao submeteu) "nao sei" tem de virar "segue em frente"; pro
    _entrou_no_composer (a cauda/comeco apareceu? entao entrou) "nao sei" NAO pode virar "nao entrou",
    senao o Enter nunca e enviado. Com dois estados isso virou regressao real: cauda curta ("ok", "sim",
    "pode fazer") devolvia False, o _entrou_no_composer lia como "nao chegou" e TODA mensagem curta
    parava de ser enviada. Achado no review, com medicao.

    Por que CAUDA e COMECO, nao so um dos dois (medido 02/08/2026, ver _RESIDUO_INICIO): o composer
    do Claude Code corta a EXIBICAO de texto longo — desenha so as primeiras linhas — entao a cauda
    fica fora da tela e nunca prova entrega nesse caso. Mas a cauda continua sendo a prova certa pro
    caso ORIGINAL (envio truncado a meio, onde sobra so o fim); os dois cobrem casos diferentes e um
    OR entre eles nao enfraquece nenhuma das duas provas — cada uma exige o mesmo minimo de
    caracteres (_RESIDUO_MIN) e a mesma comparacao exata sem espaco, so muda ONDE no texto ela olha.
    Pro _submeteu isso tambem e correto (nao so tolerado): "o comeco sumiu" e prova de submissao tao
    boa quanto "a cauda sumiu" — quando o Enter limpa o composer, os dois vao junto.

    Le a regiao do composer (entre as duas ultimas reguas) e compara com a CAUDA e o COMECO do texto
    enviado, nao com o texto todo: assim uma digitacao do usuario no composer nao vira falso positivo
    (precisa casar um trecho INTEIRO e exato do NOSSO texto, nao so ter "alguma coisa" escrito), e o
    eco da mensagem ja submetida (que fica na conversa, acima do composer) nao conta, porque so a
    regiao do composer e olhada. Pane ilegivel / sem linha de prompt -> None, nunca inventa falha: o
    custo de um falso negativo e o comportamento de hoje, o de um falso positivo e recusar envio que
    deu certo (ou pior, afirmar entrega de rascunho alheio).
    ponytail: depende do glifo ❯ do composer, igual o _READY_MARKERS depende do ⏵⏵; se um provider
    desenhar outro prompt, o upgrade e a mesma coisa — medir e acrescentar.
    """
    cauda = texto.strip().split("\n")[-1].strip()[-_RESIDUO_CAUDA:]
    inicio = texto.strip()[:_RESIDUO_INICIO]
    # Trecho curto nao acusa: "ok" ou "sim" como cauda/comeco casaria por coincidencia com o que o
    # usuario estiver digitando ao vivo no composer, e o preco de um falso positivo e o remetente
    # reenviar em cima do residuo (ou pior, o Enter submeter rascunho alheio). Sem NENHUM trecho longo
    # o bastante, degrada pro comportamento de hoje.
    cauda_curta = len(_sem_espaco(cauda)) < _RESIDUO_MIN
    inicio_curto = len(_sem_espaco(inicio)) < _RESIDUO_MIN
    if cauda_curta and inicio_curto:
        return None      # nem cauda nem comeco provam algo — nao e "nao esta"
    # Regiao do composer = entre as DUAS ULTIMAS reguas (ver _composer_regiao; as travas contra
    # pegar a regiao errada e o aviso-uma-vez de pane ilegivel moram la).
    composer = _composer_regiao(pane, nome_sessao)
    if composer is None:
        return None      # pane ilegivel: incerteza, nao ausencia
    # Paste COLAPSADO: o Claude Code recente troca paste multi-linha por "[Pasted text #N +X lines]"
    # — o texto real NUNCA aparece na tela, entao a busca pela cauda abaixo dava falso "nao chegou",
    # o Enter nao era enviado (400 'envio incompleto') e cada retry do app empilhava OUTRO paste no
    # composer (medido 31/07: mensagem quintupla entregue de uma vez quando um Enter manual drenou a
    # pilha). So conta placeholder de numero NOVO em relacao a foto pre-paste (pastes_antes): um
    # placeholder alheio (rascunho do usuario) parecer nossa entrega submeteria texto de terceiro.
    # Evidencia nos DOIS sentidos: pro _entrou_no_composer, entrou; pro _submeteu, ainda nao
    # submeteu (o placeholder some do composer com o Enter).
    if pastes_antes is not None and (_paste_ids(composer) - pastes_antes):
        return True
    # Compara SEM espaco em branco: o wrap de exibicao quebra a linha no meio da cauda/comeco (recado
    # longo de um paragrafo so passa de 200 colunas e quebra), e ai um `trecho in composer` cru falhava
    # justamente na classe de mensagem que motivou o conserto.
    composer_sem_espaco = _sem_espaco(composer)
    if not cauda_curta and _sem_espaco(cauda) in composer_sem_espaco:
        return True
    if not inicio_curto and _sem_espaco(inicio) in composer_sem_espaco:
        return True
    return False


# Teto de caracteres da regiao despejada na linha de diagnostico — cauda de log serve pra explicar
# o PROXIMO caso real (ver _diag_composer), nao pra empilhar a tela inteira no journal.
_DIAG_MAX = 400


def _diag_composer(pane: str, texto: str, name: str, pastes_antes: set[str] | None) -> str:
    """Monta a linha de diagnostico anexada aos logs de envio PARCIAL/deferred.

    So chamada no caminho de FALHA (send_prompt ja decidiu devolver partial/deferred) — nunca no
    feliz, que custaria uma captura de pane extra por envio bem sucedido. Nao pode lancar: quem
    chama ja esta lidando com um erro de entrega de verdade, e um erro AQUI dentro nao pode mascarar
    aquele. Qualquer excecao interna degrada pra uma string curta; o log perde detalhe, nunca a
    entrega.

    Junta quatro evidencias, na mesma ordem que um humano investigaria: (0) o NOME da sessao, pra
    correlacionar esta linha com as outras do mesmo envio no journal (achado da review 02/08/2026:
    o parametro chegava aqui e nunca era lido — o caller ja repete `name=%s` no format string de
    fora, mas essa repeticao e o que deixa o grep por sessao funcionar sem decorar a ordem dos
    argumentos); (1) a regiao que o detector leu como composer (ou o motivo de nao ter lido),
    truncada em _DIAG_MAX e com \\n escapado pra nao quebrar a linha do log; (2) a CAUDA e o
    COMECO procurados — os mesmos recortes que _composer_residuo usa (ver _RESIDUO_INICIO: sem o
    comeco aqui, o PROXIMO caso de "nem cauda nem comeco bateram" fica tao mudo quanto o de
    d49e87a); (3) a geometria das reguas (indices, distancia ate o fim, altura entre elas) — e o
    que diz se _COMPOSER_FUNDO/_COMPOSER_ALTURA descartaram a regiao; (4) os ids de paste vistos
    antes/depois, pro caso do texto ter colapsado em placeholder.
    """
    try:
        linhas = pane.split("\n")
        reguas = [i for i, l in enumerate(linhas) if l.count("─") >= 20]
        if len(reguas) >= 2:
            geometria = (f"reguas={reguas[-2]},{reguas[-1]} "
                         f"fundo={len(linhas) - reguas[-1]} altura={reguas[-1] - reguas[-2]}")
        else:
            geometria = f"reguas={reguas}"
        # nome_sessao="" pra este helper so LER a regiao, nunca repetir o aviso-uma-vez de composer
        # ilegivel — esse efeito colateral pertence ao caminho principal, ja disparado la se for o caso.
        regiao = _composer_regiao(pane, "")
        regiao_txt = ("ilegivel (menos de 2 reguas validas, ou fora de _COMPOSER_FUNDO/_ALTURA)"
                      if regiao is None else regiao.replace("\n", "\\n")[:_DIAG_MAX])
        cauda = texto.strip().split("\n")[-1].strip()[-_RESIDUO_CAUDA:]
        inicio = texto.strip()[:_RESIDUO_INICIO]
        pastes_depois = _paste_ids(regiao or "")
        return (f"diag: sessao={name!r} cauda={cauda!r} inicio={inicio!r} composer={regiao_txt!r} "
                f"{geometria} pastes(antes={sorted(pastes_antes or set())} depois={sorted(pastes_depois)})")
    except Exception as e:
        return f"diag indisponivel: {e!r}"


# Aviso do Pi de subagente async que a caixa de intercomunicacao nao confirmou — texto de SISTEMA,
# nao rascunho do usuario. Causa raiz medida 02/08/2026: o Pi imprime esse aviso DENTRO da mesma
# caixa do composer, e o guard antigo ("qualquer linha nao-vazia = ocupado") contava aviso como se
# fosse rascunho -> deferred pra sempre (o usuario confirmou que so destravava mexendo no terminal a
# mao). Log real:
#   " Subagent async grouped result intercom delivery was not acknowledged for
#   '/tmp/pi-subagents-uid-1000/async-subagent-results/a56523ed-40de-4fc7-a352-8fa39f29f908.json'."
#
# ACHADO DA REVIEW (mesmo dia): a 1a versao usava `[^\n]*`, exigindo a frase inteira numa LINHA
# FISICA — mas o `capture-pane` devolve linha de TELA, e o aviso tem ~167 chars: num pane estreito
# (medido em 90 e 99 colunas) ele QUEBRA no meio, `[^\n]*` para de casar na primeira quebra, e
# _composer_ocupado_pi volta a devolver True (o bug "continua adiando pra sempre" reaberto). A
# sessao nasce com `-x 200` mas o tmux segue o menor CLIENTE anexado, entao o pane estreito e real
# em uso, nao teorico. Fix: casa contra o texto com TODO espaco em branco colapsado (a mesma tecnica
# de _sem_espaco que o resto do arquivo ja usa pra sobreviver ao wrap de exibicao da cauda/comeco) —
# um wrap NUNCA acrescenta nem apaga caractere, so reposiciona o cursor pra proxima linha da tela,
# entao remover TODO whitespace (real ou introduzido pela quebra) reconstroi a sequencia de
# caracteres original em qualquer ponto de corte.
_AVISO_SUBAGENT_PI_FRASE = _sem_espaco(
    "Subagent async grouped result intercom delivery was not acknowledged for")
_AVISO_SUBAGENT_PI_RE = re.compile(
    re.escape(_AVISO_SUBAGENT_PI_FRASE) + r".*?/tmp/pi-subagents-[^']*'?\.?")
# Exige tambem o caminho `/tmp/pi-subagents-` alem da frase, pra nao casar por coincidencia um texto
# de usuario que cite palavras parecidas.
# ponytail: remendo — reconhece a FRASE especifica do aviso, nao distingue aviso de rascunho em
# GERAL (uma frase nova de um aviso futuro nao vai casar). O upgrade de verdade seria comparar o
# conteudo da caixa contra a leitura ANTERIOR: aviso de sistema e ESTATICO (nao muda entre duas
# capturas), rascunho do usuario digitando MUDA — mas isso pede duas capturas espacadas no tempo, e
# esta funcao (uma leitura so, sem estado entre chamadas) nao tem isso hoje.


def _composer_ocupado_pi(name: str) -> bool:
    """True = já ha RASCUNHO parado no composer do Pi. Digitar por cima COLARIA as mensagens num
    submit so — caso real (ABC-1234, 31/07): aviso de grupo ficou no composer com o Enter engolido
    (tmux extended-keys formato xterm), o prompt do cockpit foi digitado em cima, os dois viraram
    UMA mensagem, e o reconcile — sem achar o prompt exato no transcript — reentregou (duplicata).

    So pro Pi porque nele a leitura e deterministica (medido): composer = linhas entre as DUAS
    ULTIMAS reguas; vazio = nenhuma linha entre elas. No Claude Code o composer vazio desenha
    glifo/placeholder, entao "tem texto" nao distingue rascunho de moldura — la fica o comportamento
    de hoje. Ilegivel -> False (na duvida envia, mesma politica do resto do arquivo).

    Antes de decidir "tem rascunho?", remove o aviso de subagente conhecido (ver
    _AVISO_SUBAGENT_PI_RE) do texto da caixa, JA SEM ESPACO (sobrevive a quebra de linha do wrap de
    tela — ver comentario ali): aviso de sistema nao e rascunho do usuario, e contá-lo como ocupado
    travava o envio indefinidamente (o usuario so via a mensagem sair depois de mexer no terminal a
    mao)."""
    try:
        linhas = _capture(name).split("\n")
    except Exception:
        return False
    reguas = [i for i, l in enumerate(linhas) if l.count("─") >= 20]
    if len(reguas) < 2 or len(linhas) - reguas[-1] > _COMPOSER_FUNDO \
            or reguas[-1] - reguas[-2] > _COMPOSER_ALTURA:
        return False
    texto_composer = "\n".join(linhas[reguas[-2] + 1:reguas[-1]])
    sem_aviso = _AVISO_SUBAGENT_PI_RE.sub("", _sem_espaco(texto_composer))
    return bool(sem_aviso)


def _wait_input_ready(name: str, timeout: float | None = None, provider: str = "claude") -> bool:
    """Espera o TUI ficar interativo antes de enviar. BUG: msg mandada logo após criar a
    sessão (agente ainda bootando, TUI não aceita teclas) era ENGOLIDA -> sumia (ficava só na fila
    como bubble fantasma, o agente nunca recebia). Sessão já pronta -> retorna na 1ª leitura (sem
    latência). Timeout -> loga UMA vez, retorna False e envia mesmo assim (não piora o caso de hoje)."""
    markers = _READY_MARKERS_BY_PROVIDER.get(provider, _READY_MARKERS)
    if timeout is None:
        timeout = _TIMEOUTS_BY_PROVIDER.get(provider, _DEFAULT_TIMEOUT)
    deadline = time.monotonic() + timeout
    while True:
        # Só o rodapé (ver _READY_TAIL_LINES): no pane inteiro o marcador casa com o texto da conversa.
        if any(m in _pane_tail(_capture(name)) for m in markers):
            return True
        if time.monotonic() >= deadline:
            _warn_ready_timeout_once(name, provider, timeout)
            return False
        time.sleep(0.2)


def deliverable(name: str) -> bool:
    # Pode entregar texto livre AGORA? False se a sessao morreu (defer p/ recriacao, sem queimar 12s no
    # _wait_input_ready) ou se ha overlay/menu aberto (digitar as cegas navegaria o menu errado). Erro
    # de captura (sessao viva, pane ileg.) -> True: degrada pro envio de hoje, sem regressao.
    if not tmux.has_session(name):
        return False
    try:
        pane = _capture(name)
    except Exception:
        return True
    state, _, _, _ = classify(pane)
    return state != "awaiting_input" and not is_overlay(pane)


# Lock POR SESSAO serializando o send_prompt: dois /input quase simultaneos (ou /input + drain)
# rodavam em threads digitando no MESMO tty — o texto de B aterrissava na janela de settle de A e o
# Enter de A submetia os dois CONCATENADOS. setdefault e atomico no CPython (pior caso: Lock orfao).
_send_locks: dict[str, threading.Lock] = {}


def _send_lock(name: str) -> threading.Lock:
    return _send_locks.setdefault(name, threading.Lock())


def drain(name: str, jsonl: str, provider: str = "claude") -> int:
    """Entrega ao tty as entradas pendentes (delivered=False) quando o pane volta a aceitar texto.
    Retorna quantas entregou. claim-1-envia-1: um crash entre o claim e o envio deixa NO MAXIMO 1
    entrada 'stranded', nao o lote, e recheca o overlay (via send_prompt) a cada iteracao."""
    q = PromptQueue(name)
    # ECC: cheap-check SEM subprocess primeiro — a maioria das reconexoes nao tem pendencia; sem isto,
    # todo (re)connect dispararia um capture-pane atoa (pressao no threadpool em rajada de mobile).
    if not any(e.get("delivered") is False for e in q.load()):
        return 0
    # Poda por DUAS idades, vale a mais nova: (a) inicio do transcript (pre-/clear cria transcript
    # novo); (b) nascimento do tmux atual — sem ele, um resume (`pi -c`) reusa transcript VELHO e
    # entradas enfileiradas pra vida anterior da sessao (mesmo nome de pasta) eram entregues na
    # sessao nova. Regra do dono: sessao morreu devendo, a divida nao passa pra proxima.
    start_ts = max(_transcript_start_ts(jsonl), tmux.session_created(name))
    # Orfas de sessao anterior: nunca mais casam nem drenam — remove (senao o cheap-check acima
    # fica quente pra sempre e o lixo acumula ate o cap). A poda some com a bubble do chat, entao
    # ela nao pode ser muda: loga quantas cairam e o corte usado.
    podadas = q.prune_before(start_ts)
    if podadas:
        _log.info("poda name=%s: %d entrada(s) de vida anterior descartada(s) (corte=%.0f)",
                  name, podadas, start_ts)
    ti = TerminalInput()
    sent = 0
    while True:
        claimed = q.claim_undelivered(min_ts=start_ts, limit=1)
        if not claimed:
            return sent
        entry = claimed[0]
        try:
            # Resolve o pane NA HORA (drain roda em polls, nao no caminho quente do envio, entao
            # pagar mais um `tmux list-panes` aqui nao pesa como pesaria no /input). Pelo pane do
            # AGENTE, nao pelo ATIVO (I1 da revisao final): numa sessao Pi com split manual o ativo
            # podia ser o do shell -> `INBOX.tem_linha(pane_id)` falhava e a linha rapida do Pi se
            # perdia, o mesmo bug que a Task 4 matou no /input. MESMA resolucao do /input
            # (api._pane_info delega pra ca), nao uma quarta.
            pane_id = agentpane.pane_info(name)[1]
            # msg_id=entry["id"]: mesma identidade em TODA reentrega desta entrada (retry apos
            # "deferred" logo abaixo, ou reenvio pelo reconcile de _confirm_and_drain) — e o que
            # deixa a extensao do Pi (cp-state.ts) reconhecer um retry e nao chamar sendUserMessage
            # de novo. Ver pi_inbox.entregar.
            result = ti.send_prompt(name, entry["text"], provider, pane_id=pane_id, msg_id=entry["id"])
        except Exception:
            # Falha POS-gate (tty caiu no meio): pode ter emitido tecla -> at-most-once, NAO reverte.
            # ponytail: stranded-mas-visivel (a bubble queued- segue aparecendo, display ignora delivered);
            # upgrade: render distinto / re-drain confirmado-por-transcript se virar reclamacao real.
            return sent
        if result == "partial":
            # O texto ficou pela metade no composer e o Enter NAO foi enviado. Ate 07/08/2026 isto
            # PARAVA sem retry porque nada limpava a linha entre tentativas: reenfileirar digitaria o
            # texto inteiro EM CIMA do residuo (concatenacao, pior que a perda). Agora `_partial`
            # limpa e diz se conseguiu (via `_ULTIMA_LIMPEZA`, por THREAD — send_prompt roda nesta
            # MESMA chamada) — limpeza CONFIRMADA e a unica coisa que autoriza o requeue.
            limpou = getattr(_ULTIMA_LIMPEZA, "limpou", False)
            if hasattr(_ULTIMA_LIMPEZA, "limpou"):
                del _ULTIMA_LIMPEZA.limpou
            try:
                tentativas = q.bump_attempts(entry["id"]) if limpou else 0
                # `1 <= tentativas`, nao so `<= _PARTIAL_MAX_TENTATIVAS`: bump_attempts devolve 0 quando
                # a entrada sumiu entre o claim e aqui (ex.: um /clear esvaziou a fila no meio do
                # caminho) — sem o piso, 0 <= 2 passava, set_delivered virava no-op (a entrada nao
                # existe mais) e o log abaixo afirmava um reenfileiramento que nao aconteceu.
                if limpou and 1 <= tentativas <= _PARTIAL_MAX_TENTATIVAS:
                    q.set_delivered(entry["id"], False)
                    _log.warning("drain name=%s: envio PARCIAL da entrada %s — composer limpo, "
                                 "reenfileirado (tentativa %d/%d)", name, entry.get("id"),
                                 tentativas, _PARTIAL_MAX_TENTATIVAS)
                else:
                    # Fica delivered=True e PARA — tres motivos possiveis, cada um com o que sobra pro
                    # usuario ver DIFERENTE (item 5 da revisao final: o comentario antigo cobria so os
                    # dois primeiros com uma frase so, e a frase era falsa no segundo):
                    #  - nao limpou: o residuo esta a vista no terminal, o usuario ve as duas metades e decide;
                    #  - estourou o teto de tentativas: o composer foi limpo TAMBEM desta vez — nao sobra
                    #    residuo nenhum pra ver, so a bubble no app avisando que parou;
                    #  - a entrada sumiu da fila entre o claim e aqui (poda/`/clear` concorrente): nao ha
                    #    mais entrada pra marcar nem residuo NOSSO garantido (o clear pode ter mudado o
                    #    proprio composer) — so registra, sem afirmar nenhum dos dois casos acima.
                    if not limpou:
                        motivo, diag = "composer NAO limpo", "o residuo fica a vista no terminal"
                    elif tentativas == 0:
                        motivo, diag = ("entrada sumiu da fila antes do requeue (poda/`/clear` "
                                        "concorrente)", "sem entrada e sem residuo garantido")
                    else:
                        motivo, diag = "teto de tentativas", "composer JA limpo, nao sobra residuo pra ver"
                    _log.error("drain name=%s: envio PARCIAL da entrada %s — parado, sem retry "
                               "automatico (%s — %s)", name, entry.get("id"), motivo, diag)
            except OSError:
                # Mesma assimetria que o ramo "deferred" logo abaixo ja fecha pro dele: drain roda
                # fire-and-forget (chamado por tick do loop autonomo, hook de transicao, reconexao de
                # SSE) e uma excecao daqui subiria pro CALLER — 500 no POST /loop (api.py) ou o tick do
                # loop quebrando no meio, nenhum dos dois com guarda pra isto. Aqui LOGA em vez de so
                # `pass`: o vizinho pode calar porque a bubble queued- visivel basta como garantia; aqui
                # nao ha bubble nenhuma nova pra avisar o usuario, entao o log e a UNICA pista de que o
                # requeue (ou a desistencia) de um envio parcial nao aconteceu por causa do disco.
                _log.exception("drain name=%s: falha de disco ao (re)registrar a entrada %s apos "
                               "envio PARCIAL — nem reenfileirada nem fechada, fica como estava",
                               name, entry.get("id"))
            return sent
        if result == "deferred":
            # Reverte pra retry e para — espera o proximo idle. No caminho de TECLA isto e
            # literalmente pre-envio (send_prompt nao tocou a TUI: overlay reabriu entre claim e
            # envio). No caminho da LINHA do Pi NAO e — pi_inbox.entregar pode devolver "deferred"
            # DEPOIS de a extensao ja ter chamado sendUserMessage (ACK perdido/timeout, ou reconcile
            # reenviando um "sent" nao confirmado no transcript — achado ALTA "Porta A"/"Porta B" da
            # revisao 02/08/2026). Reenviar aqui SERIA duplicar a instrucao no agente se nao fosse
            # por uma coisa: `msg_id=entry["id"]` (linha acima) mantem o MESMO id em toda reentrega
            # desta entrada, e a extensao (cp-state.ts) guarda os ids ja entregues — um id repetido
            # so re-confirma, nunca chama sendUserMessage de novo. E o que torna o revert abaixo
            # seguro nos DOIS casos, nao so no de tecla.
            # Revert pode falhar (disco): nesse caso a entrada
            # fica delivered=True (stranded-mas-VISIVEL como bubble queued-) -> nao re-dreva, mas nao some;
            # nunca propaga (drain roda fire-and-forget no to_thread). delivered=True = "send_keys chamado",
            # nao "Claude recebeu" (tmux engole erro de envio) -> a bubble visivel e a unica garantia.
            try:
                q.set_delivered(entry["id"], False)
            except OSError:
                pass
            return sent
        sent += 1


class DriveError(RuntimeError):
    """Falha ao dirigir o picker (nav nao convergiu / review mismatch / picker preso). NADA foi
    submetido e NENHUM Escape foi mandado — o picker segue aberto; o caller (api /answer) decide o
    fallback (Escape + resposta por texto). Nao herda ValueError de proposito: ValueError = input
    invalido pre-TUI (409), DriveError = TUI nao cooperou (fallback)."""


# Linha destacada do picker: "❯ 3. OPT-TWO" -> 3 (1-based). Numerico de proposito: robusto a label
# longo/quebrado em multiplas linhas, que um match por texto erraria.
_CURSOR_ROW = re.compile(r"❯\s*(\d+)\.")


def _cursor_row(screen: str) -> int | None:
    m = _CURSOR_ROW.search(screen)
    return int(m.group(1)) if m else None


def _review_matches(screen: str, answers: list[dict]) -> bool:
    # Cada pergunta no review vira uma linha "→ <labels por ', '>". Compara por TOKEN exato (nao
    # substring) pra um label curto nao casar dentro de outra palavra.
    # ponytail: split por ',' assume que label nao contem ',' (labels do AskUserQuestion sao frases curtas)
    arrow_tokens = [
        {p.strip() for p in line.split("→", 1)[1].split(",")}
        for line in screen.splitlines() if line.strip().startswith("→")
    ]
    for a in answers:
        for lbl in a.get("labels", []):
            if not any(lbl in toks for toks in arrow_tokens):
                return False
    return True


def _validate(answers: list[dict]) -> None:
    # Valida tudo ANTES de mandar tecla: se algo falta, o TUI nunca e tocado e o ValueError vira 409
    # limpo (em vez de 500 + TUI travado no meio do caminho).
    for a in answers:
        kind = a.get("kind")
        if kind == "text":
            v = a.get("value")
            if v is None:
                raise ValueError("value required for text kind")
            if any(ord(c) < 32 and c not in "\t" for c in v):
                raise ValueError("control characters not allowed")
            if a.get("type_index") is None:
                raise ValueError("type_index required for text kind")
        elif kind == "chat":
            if a.get("chat_index") is None:
                raise ValueError("chat_index required for chat kind")
        elif kind == "option":
            if not a.get("indices"):
                raise ValueError("indices required for option kind")
        else:
            raise ValueError(f"unknown answer kind: {kind!r}")


def answer_questions(name: str, answers: list[dict]) -> None:
    """Dirige o prompt tabbed AskUserQuestion do estado INICIAL (cursor aba1/opt0) e CONFERE no Review
    antes do Submit. Input invalido -> ValueError pre-TUI (API -> 409). Drive falhou (nav nao
    convergiu / mismatch / preso) -> DriveError SEM submeter e SEM Escape — o caller manda Escape e
    reenvia a resposta como texto (fallback), pra o Escape solto nao virar "user declined".
    single = Down*idx + Enter (auto-avanca); multi = (Down ate idx + Space) por opcao, depois Right;
    texto = Down ate 'Type something' + Enter + digita + Enter; chat = Down ate 'Chat about this' + Enter."""
    _validate(answers)  # valida ANTES de tocar no TUI; loop abaixo assume input valido

    def key(k: str) -> None:
        send_keys(name, k)
        time.sleep(_SETTLE)

    for a in answers:
        kind = a.get("kind")
        if kind == "option" and not a.get("multi"):
            # single-select: desce ate o indice e Enter (TUI auto-avanca pro proximo tab)
            for _ in range(a["indices"][0]):
                key("Down")
            # Guard PRE-Enter em MALHA FECHADA: numa pergunta UNICA o Enter ja SUBMETE (nao ha tela
            # de Review depois p/ pegar drift). Um Down engolido no redraw do overlay submetia a
            # opcao errada calado. Le a linha REAL do cursor e CORRIGE (Down/Up + re-le, ate 3x) —
            # tecla engolida vira ruido auto-corrigido, nao erro. Cursor abre na linha 1 (indice 0),
            # logo esperado = indice+1. Linha ilegivel -> segue como hoje (guard so age se leu).
            # Nao convergiu -> DriveError SEM Escape (caller faz Escape + fallback por texto).
            expected = a["indices"][0] + 1
            row = _cursor_row(_capture(name))
            for _ in range(3):
                if row is None or row == expected:
                    break
                for _ in range(abs(expected - row)):
                    key("Down" if expected > row else "Up")
                row = _cursor_row(_capture(name))
            if row is not None and row != expected:
                raise DriveError(f"nav drift nao corrigido — cursor na linha {row}, esperava {expected}; nao submetido")
            key("Enter")
        elif kind == "option":
            # multi-select: para cada opcao (em ordem crescente) desce ate ela e Space; depois Right
            cur = 0
            for idx in sorted(a["indices"]):
                for _ in range(idx - cur):
                    key("Down")
                cur = idx
                key("Space")
            key("Right")
        elif kind == "text":
            # texto livre: desce ate 'Type something', Enter abre campo, digita valor, Enter submete
            for _ in range(a["type_index"]):
                key("Down")
            key("Enter")
            send_keys(name, a["value"], literal=True)  # control-char ja validado em _validate
            time.sleep(_SETTLE)
            key("Enter")
        elif kind == "chat":
            # 'Chat about this': desce ate o indice e Enter
            for _ in range(a["chat_index"]):
                key("Down")
            key("Enter")

    # Passo final depende do shape do TUI:
    #  - MULTIPLAS perguntas -> tela "Review your answers / Submit answers": confere e da Enter p/ submeter.
    #  - UNICA pergunta -> NAO ha review; o Enter da selecao ja submeteu. Sucesso, sem Escape (mandar
    #    Escape aqui interrompia o Claude que ja recebeu a resposta -> bug do "aceitou mas deu ruim").
    #  - Picker ainda aberto sem review (algo travou) -> Escape e erro, nunca submete as cegas.
    screen = _capture(name)
    if "Submit answers" in screen:
        if not _review_matches(screen, answers):
            raise DriveError("review mismatch — nao submetido")
        key("Enter")
    elif "Esc to cancel" in screen:
        raise DriveError("picker preso sem tela de review — nao submetido")
    # senao: pergunta unica ja submeteu na selecao; nada a confirmar.


# ── Picker do Pi (tool `question`) ───────────────────────────────────────────
# Cursor do picker do Pi: "> 3. label" (ascii, nao o ❯ do Claude). Nao misturar no _CURSOR_ROW
# do Claude: aquele faz search no pane INTEIRO e um "> N." citado em prosa viraria falso cursor;
# aqui o match so vale junto com o is_overlay (rodape de navegacao no FUNDO do pane), que e o que
# separa o picker vivo da citacao no scrollback.
_PI_CURSOR_ROW = re.compile(r"^\s*>\s*(\d+)\.", re.M)


def _pi_cursor_row(screen: str) -> int | None:
    rows = _PI_CURSOR_ROW.findall(screen)
    return int(rows[-1]) if rows else None   # mais ao fundo = o picker vivo


def answer_question_pi(name: str, answer: dict, question: dict) -> None:
    """Dirige o picker da tool `question` do Pi: Down/Up em malha fechada (mesmo padrao do
    answer_questions do Claude) + Enter. kind=text: navega ate o "Type something." (sempre a
    ultima linha), Enter, digita, Enter. Input invalido -> ValueError (409); drive falhou ->
    DriveError SEM submeter e SEM Escape (o caller faz Escape + fallback por texto, igual Claude)."""
    options = question.get("options") if isinstance(question.get("options"), list) else []
    kind = answer.get("kind")
    if kind == "option":
        indices = answer.get("indices") or []
        if len(indices) > 1:
            raise ValueError("multi-seleção do Pi ainda não é dirigida pelo app — responda no terminal")
        if not indices:
            raise ValueError("sem opcao escolhida")
        # labels alimentam o fallback por texto se o drive falhar — sem eles o fallback entregaria
        # nada com cara de sucesso (achado do silent-failure-hunter 2026-08-04).
        if not any(str(l).strip() for l in (answer.get("labels") or [])):
            raise ValueError("opcao sem label (o fallback por texto ficaria vazio)")
        target = int(indices[0]) + 1
        if not 1 <= target <= len(options):
            raise ValueError(f"opcao {target} fora do intervalo (1..{len(options)})")
    elif kind == "text":
        value = str(answer.get("value") or "")
        if not value.strip():
            raise ValueError("texto vazio")
        # Mesma trava do _validate do Claude: control chars nao entram no TUI.
        if any(ord(c) < 32 and c != "\t" for c in value):
            raise ValueError("texto com caractere de controle")
        target = len(options) + 1        # "Type something." e sempre a ultima linha do picker
    else:
        raise ValueError(f"kind nao suportado no picker do Pi: {kind!r}")

    screen = _capture(name)
    if not is_overlay(screen) or _pi_cursor_row(screen) is None:
        raise DriveError("picker do Pi nao esta aberto no pane")

    def key(k: str) -> None:
        send_keys(name, k)
        time.sleep(_SETTLE)

    # Nav em malha fechada. O picker estava legivel na checagem inicial: se o cursor ficar
    # ILEGIVEL no meio (capture falho devolve ""), NAO se submete as cegas — DriveError e o
    # fallback por texto assume (um Enter cego podia cair na opcao errada; o Pi nao tem Review).
    for _ in range(3):
        row = _pi_cursor_row(_capture(name))
        if row is None:
            raise DriveError("cursor do picker do Pi ficou ilegivel no meio do drive; nao submetido")
        if row == target:
            break
        for _ in range(abs(target - row)):
            key("Down" if target > row else "Up")
    else:
        raise DriveError(f"nav drift no picker do Pi — nao convergiu pra linha {target}; nao submetido")
    key("Enter")
    if kind == "text":
        time.sleep(_SETTLE)
        send_keys(name, value, literal=True)
        time.sleep(_SUBMIT_SETTLE)
        key("Enter")
    time.sleep(_OPEN_SETTLE)
    after = _capture(name)
    if not after.strip():
        time.sleep(0.5)                # um retry: capture falho bem na hora do Enter nao e veredito
        after = _capture(name)
    if not after.strip():
        raise DriveError("capture vazio apos o Enter — nao da pra confirmar a submissao")
    if is_overlay(after) and _pi_cursor_row(after) is not None:
        raise DriveError("picker do Pi ainda aberto apos o Enter — nada foi submetido")


# Rodape do picker do Kimi. Ele muda conforme o MODO e e o que distingue os tres desenhos medidos
# (13/08/2026, Kimi 0.36.0): escolha simples ("1-4 / ↵ choose"), multipla ("1-5 / ↵ toggle") e campo
# de texto ("type answer  ↵ save"). No de texto a tecla numerica vira CARACTERE — mandar numero ali
# escreveria "2" no campo em vez de escolher.
# Quantas releituras esperando a aba certa aparecer antes de desistir (o redesenho da troca de aba
# leva alguns quadros).
_KIMI_ABA_TENTATIVAS = 6
_KIMI_FOOTER_RE = re.compile(r"↵\s*(choose|toggle|save)|type answer")
_KIMI_TEXTO_RE = re.compile(r"type answer")
# Tela de Review do Kimi: sem passar por ela a resposta NAO chega na ferramenta (medido).
_KIMI_REVIEW_RE = re.compile(r"Ready to submit your answers\?")
# A pergunta desenhada AGORA. O relatorio da medicao: "pra saber qual aba esta ativa sem ler ANSI,
# use o texto depois do `? ` — ele e sempre a pergunta da aba atual" (o destaque da aba e cor, que o
# capture-pane -p descarta).
_KIMI_PERGUNTA_RE = re.compile(r"^\s*\?\s+(\S.*?)\s*$", re.M)


def _kimi_pergunta_atual(pane: str) -> str | None:
    m = _KIMI_PERGUNTA_RE.search(pane)
    return m.group(1) if m else None


def _mesma_pergunta(na_tela: str, esperada: str) -> bool:
    # Comparacao TOLERANTE: o pane quebra/corta o texto na largura da janela, entao exigir igualdade
    # exata reprovaria pergunta longa legitima. Prefixo em qualquer direcao basta pra distinguir uma
    # aba da outra, que e o unico julgamento necessario aqui.
    a = " ".join(na_tela.split())
    b = " ".join(esperada.split())
    if not a or not b:
        return False
    return a.startswith(b[:40]) or b.startswith(a[:40])


def _kimi_picker_aberto(pane: str) -> bool:
    # Pane INTEIRO, nunca a cauda: o overlay do Kimi e desenhado logo abaixo do ultimo conteudo da
    # conversa, e o resto do pane fica em branco — medido a 16, 11 e 3 linhas do fim conforme a
    # conversa cresce. Um detector de cauda perde o picker justamente na conversa curta.
    return bool(_KIMI_FOOTER_RE.search(pane))


def picker_kimi_aberto(name: str) -> bool:
    """O picker do Kimi ainda esta na tela? Usado como PROVA de que nada foi submetido quando a
    confirmacao pelo transcript estoura o prazo."""
    return _kimi_picker_aberto(_capture(name))


def answer_question_kimi(name: str, answers: list[dict], questions: list[dict]) -> None:
    """Dirige o picker de AskUserQuestion do Kimi. Input invalido -> ValueError (409); drive falhou
    -> DriveError SEM submeter (o caller faz Escape + fallback por texto, igual Claude/Pi).

    Medido no Kimi 0.36.0 e diferente dos outros dois providers em tres pontos:

    - As opcoes sao NUMERADAS na tela e a tecla numerica escolhe E JA AVANCA pra proxima aba. Nao se
      conta linha nem se manda (n-1)xDown como no Claude: numero e mais barato e nao tem drift.
    - Multi-escolha nao avanca sozinha (ali ↵ e toggle) — sai com Tab. E nao ha cursor visivel nas
      linhas, so cor, entao contar linha ali seria cego de qualquer forma.
    - "Other" e sempre a ULTIMA opcao (len(options)+1) e abre campo de texto; dali em diante numero
      vira caractere, e o rodape muda pra "type answer" — que e como se detecta esse modo.

    A CONFIRMACAO nao e visual: quem chama confere o `tool.result` daquele toolCallId no wire
    (transcript.resposta_chegou). O pane so diz o que esta desenhado; o wire diz o que a ferramenta
    recebeu."""
    if len(answers) != len(questions):
        raise ValueError(f"{len(answers)} respostas para {len(questions)} perguntas")

    def opcoes(i: int) -> list:
        o = questions[i].get("options")
        return o if isinstance(o, list) else []

    # Valida TUDO antes de tocar no terminal: um drive que para no meio deixa o picker aberto numa
    # aba qualquer, e o fallback por texto depois disso entra por cima de um overlay meio-navegado.
    plano: list[list[tuple[str, str]]] = []      # por pergunta: [(tipo, valor)]
    for i, a in enumerate(answers):
        kind, ops = a.get("kind"), opcoes(i)
        if kind == "option":
            idx = a.get("indices") or []
            if not idx:
                raise ValueError(f"pergunta {i + 1}: sem opcao escolhida")
            for n in idx:
                if not 0 <= int(n) < len(ops):
                    raise ValueError(f"pergunta {i + 1}: opcao {int(n) + 1} fora de 1..{len(ops)}")
            if len(idx) > 1 and not a.get("multi"):
                raise ValueError(f"pergunta {i + 1}: varias opcoes numa pergunta de escolha unica")
            # Em multi-escolha a tecla numerica e TOGGLE: o mesmo indice duas vezes liga e desliga, e
            # a opcao terminaria DESMARCADA — o drive seguiria ate o Submit e entregaria uma resposta
            # diferente da pedida, sem erro nenhum.
            if len(set(int(n) for n in idx)) != len(idx):
                raise ValueError(f"pergunta {i + 1}: indice repetido (a tecla numerica e toggle)")
            passos = [("tecla", str(int(n) + 1)) for n in idx]
            if a.get("multi"):
                passos.append(("tecla", "Tab"))   # ↵ ali e toggle: quem avanca e o Tab
            plano.append(passos)
        elif kind == "text":
            valor = str(a.get("value") or "")
            if not valor.strip():
                raise ValueError(f"pergunta {i + 1}: texto vazio")
            if any(ord(c) < 32 and c != "\t" for c in valor):
                raise ValueError(f"pergunta {i + 1}: texto com caractere de controle")
            # "Other" e adicionado pela propria TUI depois das opcoes do modelo.
            plano.append([("tecla", str(len(ops) + 1)), ("texto", valor), ("tecla", "Enter")])
        else:
            raise ValueError(f"pergunta {i + 1}: kind nao suportado no picker do Kimi: {kind!r}")

    if not _kimi_picker_aberto(_capture(name)):
        raise DriveError("picker do Kimi nao esta aberto no pane")

    for i, passos in enumerate(plano):
        # MALHA FECHADA por aba. Sem isto o drive so sabia "tem picker aberto" e "nao e campo de
        # texto" — nunca QUAL pergunta estava na tela. Como a tecla numerica avanca de aba sozinha,
        # um redesenho atrasado fazia a tecla da pergunta i+1 caier ainda na pergunta i, e o rodape
        # casa igual em qualquer aba do mesmo modo: nada levantava erro, o Submit saia, o
        # `tool.result` chegava e a resposta ia pra pergunta ERRADA com cara de sucesso.
        esperada = str(questions[i].get("question") or "")
        if esperada:
            for tentativa in range(_KIMI_ABA_TENTATIVAS):
                atual = _kimi_pergunta_atual(_capture(name))
                if atual and _mesma_pergunta(atual, esperada):
                    break
                time.sleep(_SETTLE)     # redesenho da troca de aba pode estar em voo
            else:
                raise DriveError(
                    f"a tela nao chegou na pergunta {i + 1} ({esperada[:40]!r}); nao submetido")
        for tipo, valor in passos:
            pane = _capture(name)
            if not _kimi_picker_aberto(pane):
                raise DriveError(f"picker do Kimi sumiu na pergunta {i + 1}; nao submetido")
            # Numero so vale enquanto o rodape NAO estiver em modo texto — ali ele viraria caractere.
            if tipo == "tecla" and valor.isdigit() and _KIMI_TEXTO_RE.search(pane):
                raise DriveError(f"picker do Kimi em modo texto na pergunta {i + 1}; nao submetido")
            if tipo == "texto":
                send_keys(name, valor, literal=True)
                time.sleep(_SUBMIT_SETTLE)
            elif valor.isdigit():
                send_keys(name, valor, literal=True)   # literal: "1" e caractere, nao nome de tecla
                time.sleep(_SETTLE)
            else:
                send_keys(name, valor)
                time.sleep(_SETTLE)

    time.sleep(_OPEN_SETTLE)
    review = _capture(name)
    if not _KIMI_REVIEW_RE.search(review):
        # Sem Review na tela nao se aperta nada: um "1" as cegas podia cair numa pergunta que ficou
        # aberta e escolher a opcao errada.
        raise DriveError("tela de Review do Kimi nao apareceu; nada submetido")
    send_keys(name, "1", literal=True)                 # [1] Submit
    time.sleep(_OPEN_SETTLE)


# Teto de teclas na limpeza. C-u apaga UMA linha do composer (medido 07/08/2026: um bloco de 4
# linhas precisou de 6 envios; uma colagem colapsada em "[Pasted text #N]" sai com um so). O teto
# existe porque nem toda TUI honra C-u — num pane de shell ele volta rc=0 sem apagar nada, e sem
# teto isto viraria laco infinito no executor de envio.
_LIMPEZA_MAX_TECLAS = 12

# Teto de requeues de um envio parcial: ate 2 (ou seja ate 3 envios reais: o original + 2 requeues).
# Mesmo teto efetivo do reconcile (pqueue.reconcile_delivered, max_attempts=2) — os dois permitem
# exatamente 2 requeues, apesar de comparacoes DIFERENTES. Parcial compara POS-incremento
# (`bump_attempts` incrementa e devolve o novo valor), reconcile compara PRE-incremento (`attempts
# >= max_attempts` em pqueue.py:341 antes de somar). A diferenca de contagem cancela a diferenca de
# comparacao: mesmo teto. Unica divergencia real e o numero gravado POS-desistencia (3 no parcial,
# 2 no reconcile) — residuo de contabilidade, nao teto.
#
# Os dois leem/escrevem o MESMO campo `attempts` da entrada — nao sao contadores independentes.
# Consequencia real: uma entrada que gastou os 2 requeues do PARCIAL (attempts chegou a 2) e so
# DEPOIS conseguiu um envio de verdade (delivered=True) nunca mais e reconciliada — reconcile_delivered
# le `attempts>=2` de cara na primeira checagem, sem ter requeuado nenhuma vez por conta propria,
# e desiste na hora (confirmed=True) sem re-tentar. Comportamento aceito (quem for mexer nos dois
# tetos de novo precisa lembrar que estao no MESMO campo).
_PARTIAL_MAX_TENTATIVAS = 2

# Resultado da ULTIMA limpeza, lido pelo `drain` (mesma chamada, logo apos o send_prompt que
# devolveu "partial") pra decidir se pode reenfileirar. POR THREAD, nao por sessao: o `_send_lock`
# que protege a escrita (dentro de send_prompt) e por NOME e e solto assim que send_prompt retorna
# — ANTES do drain ler este valor. api.py:1314 documenta que POST /input e drain podem correr
# concorrentes pra MESMA sessao, entao um dict chaveado por nome tinha corrida real: thread A sai de
# send_prompt com "partial", thread B (outro envio pra mesma sessao) entra, tambem da partial e
# sobrescreve a chave antes de A ler — A reenfileiraria uma limpeza que nao foi a dela (residuo por
# baixo do requeue, o mesmo bug que isto existe pra matar). threading.local fecha isso: drain sempre
# chama send_prompt na MESMA thread, sincronamente, entao so ela ve o que ela mesma escreveu.
_ULTIMA_LIMPEZA = threading.local()


def _limpar_composer(name: str, texto: str, pastes_antes: set[str] | None) -> bool:
    """Tira do composer o texto que NOS digitamos e nao conseguimos submeter.

    True = havia residuo NOSSO e ele saiu (confirmado por leitura). False = nao havia, nao era
    nosso, ou nao saiu.

    Existe porque devolver "partial" deixando o texto la faz o reenvio digitar por cima: um Enter
    depois submete as duas copias grudadas — foi o "ne?e eu testei" de 07/08/2026.

    Duas regras que NAO podem ser afrouxadas:
      1. so age com `is True` — `_composer_residuo` e TRI-ESTADO e `None` quer dizer "nao da pra
         saber" (pane ilegivel, ou texto com menos de _RESIDUO_MIN caracteres sem espaco). Tratar
         None como "esta limpo" devolveria True sem ter limpado, e o requeue reenviaria em cima do
         residuo — exatamente a concatenacao que isto vem matar;
      2. so limpa o que e NOSSO — o dono pode ter digitado no terminal na janela em que o envio
         falhou, e apagar a frase dele e pior que a duplicata.

    Limitacao assumida: mensagem curta (< _RESIDUO_MIN caracteres sem espaco) nunca e reconhecida,
    entao nunca e limpa. Ali o comportamento continua o de hoje.
    """
    if _composer_residuo(_capture(name), texto, name, pastes_antes) is not True:
        return False
    for _ in range(_LIMPEZA_MAX_TECLAS):
        send_keys(name, "C-u")
        if _composer_residuo(_capture(name), texto, name, pastes_antes) is False:
            return True                          # False = "olhei e nao esta la". None NAO serve.
    _log.warning("composer de %r nao limpou apos %d x C-u — o texto parcial ficou la; um reenvio "
                 "vai digitar por cima", name, _LIMPEZA_MAX_TECLAS)
    return False


def _partial(name: str, motivo: str, texto: str, pastes_antes: set[str] | None = None) -> str:
    """Unico ponto que devolve "partial": loga com o diagnostico do composer, limpa e registra se a
    limpeza pegou (o `drain` le isso pra decidir entre reenfileirar e parar).

    Uma funcao so para os seis sites porque o conserto e o mesmo em todos — limpar em cada caller
    daria seis chances de esquecer um, e foi assim que o residuo sobreviveu ate agora.
    """
    _log.error("envio PARCIAL name=%s: %s — %s", name, motivo,
               _diag_composer(_capture(name), texto, name, pastes_antes))
    _ULTIMA_LIMPEZA.limpou = _limpar_composer(name, texto, pastes_antes)
    return "partial"


# Marcador da fila INTERNA da TUI do Kimi ("↑ to edit · ctrl-s to steer immediately", medido em
# 19/08/2026 no 0.37.2, some no instante em que a fila é promovida): presente = há o que steerar;
# ausente = o ctrl-s seria no-op e o caller precisa saber disso (chip da UI fica ou sai).
_STEER_MARKER = "ctrl-s to steer"


def steer_now(name: str) -> bool | str:
    """`ctrl-s` AVULSO: promove o que JA esta na fila da TUI do Kimi pro turno em curso.

    Medido em 14/08/2026 numa sessao Kimi real: texto+Enter com ele trabalhando deixa a msg na fila
    DELE ("↑ to edit · ctrl-s to steer immediately" embaixo dela no terminal); o `ctrl-s` a injeta
    no turno que ja esta rodando — vira `turn.steer` no wire.jsonl, no MESMO turnId, junto com o
    `context.append_message` de user de sempre (por isso o dedup da fila do app nao muda nada).

    Tecla avulsa, e nao um parametro do envio, DE PROPOSITO: o momento em que o dono decide "essa
    nao espera" e depois de ja ter mandado — o app mostra a fila e oferece a saida, ele escolhe.

    Dentro do `_send_lock` da sessao: senao esta tecla pode cair no MEIO de um envio digitando
    (entre o texto e o Enter), e ai ela steera a msg ANTERIOR e deixa a nova pela metade no composer.

    Sem nada na fila e no-op medido (14/08/2026: pane nao muda, teclado segue respondendo) — e e
    por isso que o marcador e checado ANTES: devolve "sem-fila" sem teclar nada, pro caller nao
    confirmar entrega de uma promocao que nao aconteceu. A checagem de marcador degrada pra tecla
    cega se o pane estiver ilegivel (comportamento de sempre), nunca bloqueia.

    Retorno TRI-ESTADO: False = o tmux RECUSOU a tecla (pane morto, sessao caiu) — sem repassar
    isso, a rota respondia 200 pra uma tecla que nunca saiu, o chip sumia da tela e a msg ficava
    parada na fila sem ninguem saber. "sem-fila" = a TUI nao tinha o que promover. True = promovido
    (o caller baixa a fila duravel)."""
    with _send_lock(name):
        try:
            if _STEER_MARKER not in (_capture(name) or ""):
                return "sem-fila"
        except Exception:
            pass  # pane ilegivel: degrada pro comportamento de sempre (tecla e no-op se vazio)
        return send_keys(name, "C-s") is not False


class TerminalInput:
    def send_prompt(self, name: str, text: str, provider: str = "claude",
                    pane_id: str | None = None, msg_id: str | None = None) -> str:
        # msg_id: SO importa pro caminho da linha do Pi (repassado a entregar_sync abaixo) — nada
        # mais neste metodo le esse valor. Quem chama sem id estavel (nenhuma PromptQueue por perto)
        # simplesmente nao passa; o caminho de tecla (Claude/Codex/Pi sem linha) nunca soube o que e
        # isso e continua identico. Ver pi_inbox.entregar sobre POR QUE ele existe.
        # Surrogate solto (meio emoji cortado pelo browser) tambem nao chega no tmux: o argv do
        # subprocess e encodado em utf-8 e estouraria UnicodeEncodeError — um ValueError, que o
        # caller ja traduz pra 400 "control characters". A msg era recusada com erro trocado e
        # nunca entrava na fila. Troca por U+FFFD como no resto do app (ver app.models).
        text = scrub_surrogates(text)
        # Validacao PRE-envio: input ruim nunca toca a TUI nem entra na fila. \n/\t ok; outros controles nao.
        if any(ord(c) < 32 and c not in "\t\n" for c in text):
            raise ValueError("control characters not allowed in prompt")
        # Serializa por sessao (gate + digitacao + Enter como unidade): sem o lock, envios
        # concorrentes intercalavam teclas no mesmo tty e as mensagens saiam concatenadas.
        with _send_lock(name):
            # Sessão Pi com a extensão conectada: entrega por chamada de função dentro do processo
            # do Pi, sem digitar e sem ler a tela — a remoção da causa raiz dos bugs de 01-02/08.
            #
            # ANTES do gate `deliverable`: aquele gate existe pra não digitar às cegas num overlay,
            # e aqui não se digita nada. Depois dele, sessão Pi com picker na tela nunca tentaria a
            # linha e ficaria adiando — exatamente o bug que isto vem matar.
            #
            # "sem-linha" é o ÚNICO retorno que autoriza continuar pra tecla: depois de ter mandado
            # pela linha, digitar por cima entregaria a mesma instrução duas vezes.
            #
            # ponytail: JANELA RESIDUAL CONHECIDA (registrada, nao fechada agora). Esta e a SEGUNDA
            # checagem de tem_linha — a primeira roda em api.py (_send_one, decide se pre-cria a
            # entrada da fila com msg_id) ANTES de disputar o _send_lock acima. As duas nao
            # compartilham trava (claim_undelivered do drain, em pqueue.py, usa so o _append_lock,
            # sem relacao com este _send_lock). Se a linha cair ENTRE a checagem de api.py e esta
            # aqui, quem chega neste ponto ve tem_linha=False mesmo com uma entrada ja criada com
            # msg_id — e cai pro fluxo de tecla abaixo, que NUNCA le msg_id (comentario no topo do
            # metodo). Resultado: saiu pela linha de um lado, redigitado do outro. Nao e regressao
            # deste commit — e o buraco original encolhido de "qualquer sessao Pi" pra "sessao com
            # linha viva no instante do append em api.py, e a linha caiu bem nessa janela". Medido:
            # entregar_sync segura o _send_lock por ate PRAZO_ACK+2.0 = 5s (pi_inbox.py), janela de
            # segundos — tempo de sobra pra um drain de reconexao de SSE ou de transicao de hook
            # entrar.
            # CUIDADO no upgrade: so mover o append() de api.py pra dentro deste _send_lock fecha a
            # corrida entre as DUAS LEITURAS de tem_linha() (o TOCTOU vira leitura unica) mas NAO
            # fecha a duplicata. A entrada nasce delivered=False no append() e so vira True quando o
            # set_delivered(...) do fim de _send_one roda DEPOIS que send_prompt() (este metodo)
            # retorna — tambem fora de qualquer trava. Nesse intervalo (que inclui a espera inteira
            # por este _send_lock MAIS os ate 5s do entregar_sync) a entrada continua reivindicavel
            # por claim_undelivered. Upgrade completo precisa das DUAS coisas juntas: o append() E o
            # set_delivered() final dentro da MESMA trava — ou claim_undelivered passar a
            # respeitar/disputar o _send_lock. Mover so o append() e necessario, mas sozinho e
            # insuficiente.
            if provider == "pi" and pane_id and pi_inbox.INBOX.tem_linha(pane_id):
                r = pi_inbox.INBOX.entregar_sync(pane_id, text, msg_id)
                if r != "sem-linha":
                    return r
            # Gate de entregabilidade (chokepoint UNICO p/ texto livre — /input e drain passam por
            # aqui): nao digitar as cegas num overlay (AskUserQuestion/picker), as teclas o
            # corromperiam. Sem pane entregavel agora, devolve "deferred" SEM tocar a TUI; o caller
            # enfileira pendente e o drain entrega quando o overlay fechar / a sessao voltar.
            if not deliverable(name):
                if not tmux.has_session(name):
                    # Sessao morreu de vez: o nome nunca mais volta, entao os contadores de deferred
                    # (deste motivo e do composer-ocupado do Pi) so cresceriam pra sempre. Achado da
                    # review 02/08/2026.
                    _limpa_deferred(name, _OCUPADO_WARNED, _OCUPADO_DEFER_COUNT)
                    _limpa_deferred(name, _INDISPONIVEL_WARNED, _INDISPONIVEL_DEFER_COUNT)
                    return "deferred"
                # Sessao viva mas indisponivel AGORA (overlay/menu aberto, ou awaiting_input — ver
                # `deliverable`). Ate a review 02/08/2026 este era o UNICO deferred do arquivo sem
                # log nenhum: overlay preso virava todo envio em adiamento silencioso pra sempre.
                _avisa_deferred(name, "sessao indisponivel (overlay/menu aberto no terminal)",
                                _INDISPONIVEL_WARNED, _INDISPONIVEL_DEFER_COUNT,
                                _diag_composer(_capture(name), text, name, None))
                return "deferred"
            _limpa_deferred(name, _INDISPONIVEL_WARNED, _INDISPONIVEL_DEFER_COUNT)
            # Não enviar pra um TUI ainda bootando: as teclas seriam engolidas e a msg sumiria (core
            # bug — msg mandada logo após criar a sessão nunca chegava no claude).
            _wait_input_ready(name, provider=provider)
            # Composer do Pi ja com texto (residuo de Enter engolido / rascunho / parcial anterior):
            # adia em vez de digitar por cima — deferred reverte pra delivered=False e o proximo
            # drain (idle/reconnect) tenta de novo; a bubble queued- segue visivel no app.
            if provider == "pi" and _composer_ocupado_pi(name):
                _avisa_deferred(name, "composer do pi ja tem texto", _OCUPADO_WARNED,
                                _OCUPADO_DEFER_COUNT, _diag_composer(_capture(name), text, name, None))
                return "deferred"
            _limpa_deferred(name, _OCUPADO_WARNED, _OCUPADO_DEFER_COUNT)
            if "\n" in text:
                # Foto dos placeholders de paste ANTES do nosso: so um numero NOVO conta como
                # evidencia de entrega (ver _composer_residuo — paste alheio nao pode virar prova).
                regiao_antes = _composer_regiao(_capture(name), name)
                pastes_antes = _paste_ids(regiao_antes or "")
                # Windows + Claude: o clipboard e o unico caminho que entrega multi-linha inteiro (o
                # linha-a-linha mede 309 de 600 e devolve "sent"). Gated em `claude` porque `Alt+V` e
                # binding DELE — Pi e Codex tem resposta propria (Codex nem passa aqui: usa o
                # app-server).
                if os.name == "nt" and provider == "claude":
                    # Composer ilegivel na hora da foto: `pastes_antes` sairia como conjunto VAZIO, e
                    # vazio nao quer dizer "nao havia placeholder", quer dizer "nao consegui olhar".
                    # Com isso um `[Pasted text #N]` que JA estava la (rascunho do dono) contaria como
                    # novo e viraria prova da NOSSA entrega — e neste caminho a prova e o unico gate
                    # antes do Enter, sem fallback nenhum atras. No caminho de sempre isso e tolerado
                    # porque ha outras evidencias; aqui nao ha.
                    if regiao_antes is None:
                        return _partial(name, "composer ilegivel antes de colar — sem foto dos "
                                              "placeholders nao ha como provar entrega", text, None)
                    # O lock e segurado da escrita ATE o fim da prova. Soltar antes reabre a janela em
                    # que outra sessao sobrescreve o clipboard e o nosso M-v cola o texto dela — com
                    # um `[Pasted text #N]` novo aparecendo do mesmo jeito, entao a prova nao ve.
                    with tmux._CLIP_LOCK:
                        if not tmux.paste_via_clipboard(name, text):
                            return _partial(name, "clipboard nao escrito — nada foi digitado no "
                                                  "pane", text, pastes_antes)
                        if not _provou_entrega(name, text, pastes_antes):
                            # Sem fallback pro caminho antigo DE PROPOSITO: ele e justamente o que
                            # perde 291 de 600 linhas afirmando entrega. Aqui vira partial, que limpa
                            # o composer e deixa a fila reenfileirar.
                            return _partial(name, "colagem pelo clipboard sem prova no composer — "
                                                  "Enter NAO enviado", text, pastes_antes)
                    send_keys(name, "Enter")
                    if not _submeteu(name, text, pastes_antes):
                        return _partial(name, "colagem submetida mas a cauda continua no composer",
                                        text, pastes_antes)
                    return "sent"
                # `is False` e nao `not ...` — MESMO raciocinio do ramo de uma linha logo abaixo: o
                # UNICO produtor de False e uma falha CONFIRMADA (ver tmux.paste_text/
                # _paste_linha_a_linha/_send_literal); None (dublê de teste que nao repassa sinal)
                # segue o caminho de sempre.
                # CRITICO (review 02/08/2026): antes deste conserto o retorno de `paste_text` era
                # DESCARTADO aqui, e a UNICA prova de entrega passou a ser a leitura da tela — que,
                # com o comeco valendo como evidencia (_RESIDUO_INICIO), enxerga "entrou" mesmo
                # quando o paste parou no MEIO (ex.: linha 2 de 3 falhou e as demais nunca foram
                # digitadas). Resultado: _entrou_no_composer dizia "entrou", o Enter ia, _submeteu via
                # o composer limpar (o Enter limpou o texto TRUNCADO), e send_prompt devolvia "sent"
                # CALADO pra um texto pela metade — no fallback do Windows (psmux sempre cai la) e no
                # POSIX quando o paste-buffer falha e cai no mesmo fallback. Checar o retorno aqui
                # tira essa decisao das maos da leitura de tela sempre que ha um sinal melhor.
                if tmux.paste_text(name, text) is False:
                    return _partial(name, "multi-linha PAROU no meio da digitacao (falha "
                                    "confirmada em tmux.paste_text) — Enter nao enviado",
                                    text, pastes_antes)
                # Settle ANTES do Enter, como no ramo de uma linha. Ver _MULTILINE_SUBMIT_SETTLE:
                # os 0.05 antigos eram menores que a ingestao MINIMA medida (0.08s) e o Enter
                # submetia o texto pela metade.
                time.sleep(_MULTILINE_SUBMIT_SETTLE)
                if not _entrou_no_composer(name, text, pastes_antes):
                    # NAO aperta Enter: o texto nao chegou no composer, entao o Enter submeteria o que
                    # estivesse la (a primeira linha truncada, ou nada) como se fosse pedido do usuario.
                    return _partial(name,
                                    f"multi-linha NAO chegou no composer em {_SUBMIT_CHECK_PRAZO:.1f}s "
                                    "(o multiplexador aceitou e nao entregou) — Enter nao enviado",
                                    text, pastes_antes)
                send_keys(name, "Enter")
                # CONFERE em vez de confiar no settle. Caso real medido: tres recados longos
                # cross-server sairam com delivered=True e NUNCA viraram entrada no transcript do
                # destino — ficaram com attempts=2 na fila (requeue duas vezes e desistencia), e o
                # dono do outro lado so os achou lendo o sidecar. Um settle maior reduz a chance e nao
                # detecta nada: o Enter correndo a ingestao devolve "sent" do mesmo jeito.
                if not _submeteu(name, text, pastes_antes):
                    return _partial(name,
                                    f"multi-linha nao submeteu (a cauda do texto continua no "
                                    f"composer apos {_SUBMIT_CHECK_PRAZO:.1f}s) — nao afirmando entrega",
                                    text, pastes_antes)
            elif text.lstrip().startswith("/"):
                # Slash command: ao digitar "/..." o Claude Code abre um menu de autocomplete. Sem dar
                # tempo do menu renderizar, o Enter corre com o redraw e e ENGOLIDO pelo menu (o comando
                # fica digitado mas NAO executa -> "o slash nao chega no terminal"). Espera o menu
                # acomodar, Enter pra executar; um 2o Enter cobre o caso do 1o so ter selecionado a
                # sugestao (o comando ja rodou e o prompt esta vazio -> o 2o Enter e no-op inofensivo).
                send_keys(name, text, literal=True)
                time.sleep(_SLASH_SETTLE)
                send_keys(name, "Enter")
                time.sleep(_SLASH_SETTLE)
                send_keys(name, "Enter")
            else:
                # Foto dos placeholders de paste ANTES do nosso, igual ao ramo multi-linha (ver
                # comentario la em cima e _composer_residuo). SEM ela a evidencia por placeholder fica
                # DESLIGADA (_composer_residuo so aceita quando pastes_antes != None) — medido 01/08:
                # acima de ~800 chars numa linha so, o Claude Code colapsa o texto em
                # "[Pasted text #N ...]" e o texto real nunca e desenhado na tela, entao a busca pela
                # cauda visivel abaixo falhava SEMPRE. Era esse o furo por tras do "envio incompleto"
                # de ditado por voz (que vira uma linha so de 1000+ chars): o Enter nunca era mandado.
                pastes_antes = _paste_ids(_composer_regiao(_capture(name), name) or "")
                # `is False` e nao `not ...`: o UNICO produtor de False e o tmux._send_literal quando o
                # fatiamento para no meio. Qualquer outro retorno (True, ou None de um dublê/wrapper que
                # nao repassa) segue o caminho de sempre — o sinal aqui e "provadamente parcial", nao
                # "nao deu True", senao um None inocente cancelaria o Enter de um envio que deu certo.
                if send_keys(name, text, literal=True) is False:
                    # Envio parou no meio (só acontece no fatiamento do Windows — ver tmux._send_literal).
                    # NÃO manda Enter: submeter texto com buraco faria a sessão agir sobre um pedido que
                    # o usuário nunca escreveu. Devolve "partial" pro caller reportar em vez de afirmar
                    # entrega — via `_partial`, que agora também limpa o composer antes de devolver.
                    return _partial(name, "texto ficou pela metade no input, Enter NAO enviado",
                                    text, pastes_antes)
                # Settle ANTES do Enter: sem isto o Enter corria a ingestao do texto e o claude (que
                # detecta input rapido como paste) tratava o Enter como parte do conteudo -> o texto
                # ficava no input SEM submeter (usuario tinha que reenviar). Espelha o gap multiline.
                # ponytail: settle fixo; se ainda escapar em device lento, upgrade = capturar o pane e
                # reenviar Enter se o input nao limpou.
                time.sleep(_SUBMIT_SETTLE)
                if not _entrou_no_composer(name, text, pastes_antes):
                    return _partial(name,
                                    f"o texto NAO chegou no composer em {_SUBMIT_CHECK_PRAZO:.1f}s "
                                    "— Enter nao enviado", text, pastes_antes)
                send_keys(name, "Enter")
                # Mesma conferencia do ramo multi-linha: e o upgrade que o comentario acima ja anotava
                # ("capturar o pane e reenviar Enter se o input nao limpou"). Aqui em vez de reenviar
                # Enter as cegas a gente REPORTA — reenviar podia submeter texto que o usuario digitou
                # no composer no meio do caminho.
                if not _submeteu(name, text, pastes_antes):
                    return _partial(name,
                                    f"uma linha nao submeteu (texto continua no composer apos "
                                    f"{_SUBMIT_CHECK_PRAZO:.1f}s) — nao afirmando entrega",
                                    text, pastes_antes)
            return "sent"

    # Teclas de navegacao liberadas pro espelho do pane (TerminalMirror dirige overlays so-TUI:
    # /status, /config, /help, pickers). Allowlist (nao texto livre) pra so passar navegacao -> nada
    # de control chars arbitrarios na TUI. Valor = nome de tecla do tmux send-keys (PPage/NPage =
    # PageUp/PageDown; BTab = Shift-Tab).
    _NAV_KEYS = {
        "Up": "Up", "Down": "Down", "Left": "Left", "Right": "Right",
        "Enter": "Enter", "Escape": "Escape", "Tab": "Tab", "BTab": "BTab",
        "PageUp": "PPage", "PageDown": "NPage", "Space": "Space",
    }

    def send_key(self, name: str, key: str) -> None:
        # Manda UMA tecla de navegacao (allowlist) pro pane. Usado pelo espelho do pane.
        tmux_key = self._NAV_KEYS.get(key)
        if tmux_key is None:
            raise ValueError(f"key not allowed: {key!r}")
        send_keys(name, tmux_key)

    # Terminal INTERATIVO (so desktop): alem da navegacao, edicao de linha + control-chars de
    # shell/TUI. Texto livre vai literal (send_text); teclas nomeadas por esta allowlist.
    _TERM_KEYS = {
        **_NAV_KEYS,
        "Backspace": "BSpace", "Delete": "DC", "Home": "Home", "End": "End",
        "C-c": "C-c", "C-d": "C-d", "C-r": "C-r", "C-u": "C-u", "C-k": "C-k",
        "C-w": "C-w", "C-a": "C-a", "C-e": "C-e", "C-l": "C-l", "C-z": "C-z",
        "C-p": "C-p", "C-n": "C-n", "C-b": "C-b", "C-f": "C-f", "C-g": "C-g",
    }

    def send_text(self, name: str, text: str) -> None:
        # Texto digitado no terminal desktop. Literal -> tmux nao interpreta como nome de tecla.
        if text:
            send_keys(name, text, literal=True)

    def send_term_key(self, name: str, key: str) -> None:
        tmux_key = self._TERM_KEYS.get(key)
        if tmux_key is None:
            raise ValueError(f"key not allowed: {key!r}")
        send_keys(name, tmux_key)

    def select(self, name: str, option: int) -> None:
        if option < 1:
            raise ValueError("option must be >= 1")
        for _ in range(option - 1):
            send_keys(name, "Down")
        send_keys(name, "Enter")

    def interrupt(self, name: str, clear: bool = False) -> None:
        # Esc UNICO = interrompe o Claude MAS mantem o texto enfileirado no input (doc oficial). Por isso
        # o proximo envio digitava EM CIMA do residuo -> concatenava. clear=True manda um 2o Esc: com o
        # input nao-vazio (garantido pelo caller — so passa clear quando havia msg pendente) o Esc-Esc
        # limpa o draft. NUNCA mandar o 2o Esc as cegas: input vazio + Esc-Esc abre o menu de rewind.
        send_keys(name, "Escape")
        if clear:
            time.sleep(_SETTLE)  # deixa o interrupt assentar e o texto voltar pro input antes de limpar
            send_keys(name, "Escape")

    # Intervalo entre as duas capturas que separam spinner VIVO de marcador congelado. Precisa ser
    # > 1s: o texto do spinner carrega os segundos decorridos ("✻ Crunched for 24s"), entao um
    # intervalo curto le o mesmo texto duas vezes e chamaria de parada uma sessao trabalhando.
    _SPIN_GAP = 1.2
    # Prazo pra linha `⎿ Set model to …` aparecer depois do Enter (medido: passa de 1s).
    _RESULT_PRAZO = 4.0
    # Prazo pro picker aparecer depois do Enter, ANTES de arriscar um segundo Enter.
    _OPEN_PRAZO = 2.0

    class NaoDigitou(mp.PickerError):
        """Recusa ANTES de qualquer tecla ir pro tty.

        Existe pra quem chama saber que o terminal ficou intocado: a rota de troca de modelo espera
        ate 3.6s pela escrita do settings.json aterrissar antes de repor o valor anterior, e pagar
        essa espera numa requisicao que nem chegou a digitar so faz o erro demorar a aparecer.
        """

    def _require_drivable(self, name: str) -> None:
        """Recusa digitar `/model` quando a sessao nao pode receber comando AGORA.

        Com o Claude trabalhando, o texto digitado nao vira comando: cai no campo de entrada e o
        Enter o ENFILEIRA como mensagem — a troca de modelo virava um "/model" mandado pro Claude
        ler. Overlay aberto (outro menu, /status) navegaria o menu errado. Guard no caminho
        COMPARTILHADO de abrir o picker, pra valer pros tres drivers (listar, trocar, esforco).

        DUAS capturas, nao uma: um pane parado nao distingue spinner vivo de marcador de turno
        concluido — os dois renderizam "<glifo> <palavra> for <N>s" (esta na docstring do
        state.classify). Uma captura so recusava, com "esta trabalhando", uma sessao que tinha
        acabado de terminar e ficou com o "✻ Crunched for 24s" na tela; medido numa sessao real.
        Quem decide e a ANIMACAO: o texto mudou entre as duas leituras -> vivo.
        """
        if not tmux.has_session(name):
            raise self.NaoDigitou(409, "sessao nao esta viva")
        try:
            pane = _capture(name)
        except Exception:
            return  # pane ilegivel: degrada pro comportamento de hoje em vez de travar a tela
        if is_overlay(pane):
            raise self.NaoDigitou(409, "ha um menu aberto no terminal da sessao")
        spin = _live_spinner(pane)
        if spin is None:
            return
        time.sleep(self._SPIN_GAP)
        try:
            depois = _capture(name)
        except Exception:
            return
        if is_overlay(depois):
            raise self.NaoDigitou(409, "ha um menu aberto no terminal da sessao")
        if _live_spinner(depois) != spin:
            raise self.NaoDigitou(409, "a sessao esta trabalhando — espere ela terminar")

    def _open_model_picker(self, name: str) -> str:
        """Abre o picker do `/model` e devolve o pane com ele aberto.

        Se o autocomplete engolir o 1o Enter, um 2o submete (guardado por picker_open pra nunca
        mandar Enter num picker ja aberto, o que confirmaria como default). Falhou -> Esc e
        PickerError, pra nunca deixar o picker preso.
        """
        self._require_drivable(name)
        send_keys(name, "/model", literal=True)
        time.sleep(_SETTLE)
        send_keys(name, "Enter")
        pane = self._espera_picker(name)
        if pane is None:
            # So agora o 2o Enter. INSISTIR na leitura antes disso nao e capricho: se o picker JA
            # abriu e a captura pegou o redraw pela metade, esse Enter confirma a linha sob o cursor
            # COMO DEFAULT — troca o modelo padrao do usuario num caminho que era pra ser somente
            # leitura (a folha busca a lista sozinha ao abrir). Uma foto unica dava essa chance a
            # cada abertura; o poll tira o sorteio da jogada.
            send_keys(name, "Enter")
            pane = self._espera_picker(name)
        if pane is None:
            self._abort(name)
            raise mp.PickerError(409, "model picker did not open")
        return pane

    def _espera_picker(self, name: str) -> str | None:
        """Pane com o picker INTEIRO desenhado, ou None se ele nao apareceu dentro do prazo.

        Espera o rodape, nao so o titulo: no instante em que o titulo aparece as linhas de modelo
        ainda estao sendo pintadas, e ler ali devolvia a lista incompleta (medido: 4 modelos, sem o
        Haiku, que e a ultima linha).
        """
        fim = time.monotonic() + self._OPEN_PRAZO
        while True:
            time.sleep(_SETTLE)
            pane = tmux.capture_pane(name)
            if mp.picker_desenhado(pane):
                return pane
            if time.monotonic() >= fim:
                # Titulo na tela mas rodape nao: o picker ABRIU (so nao terminou de pintar). Devolve
                # assim mesmo — mandar um 2o Enter aqui e o pior desfecho possivel, porque ele
                # confirmaria a linha sob o cursor como default.
                return pane if mp.picker_open(pane) else None

    def list_model_options(self, name: str) -> dict:
        """Le as linhas do picker do `/model` e o fecha com Esc, sem aplicar nada.

        E a fonte da lista que a tela mostra pra sessao da CONTA ANTHROPIC: chumbar os modelos no
        front ja tinha ficado velho (o Fable entrou no picker e sumiu da tela do app). O picker e
        um overlay — nao vai pro scrollback —, entao abrir e fechar nao deixa rastro na conversa
        nem gasta token.
        """
        with _send_lock(name):
            pane = self._open_model_picker(name)
            rows = mp.parse_model_rows(pane)
            effort = mp.parse_current_effort(pane)
            self._abort(name)
        return {"models": rows, "effort": effort}

    def set_engine_model(self, name: str, model_id: str) -> dict:
        """Troca o modelo de uma sessao de MOTOR digitando `/model <id>`.

        Nao da pra usar o picker aqui: numa sessao de motor ele lista so os 4 aliases, todos
        apontando pro mesmo `ANTHROPIC_MODEL` (medido). O comando com argumento aceita id
        arbitrario — e, junto, grava o id como default global; quem desfaz isso e o
        `default_model.restore()` do lado da rota, nao este driver.
        """
        alvo = model_id.strip()
        # Vai virar TECLA no tty: um espaco quebraria o argumento em dois e um caractere de
        # controle poderia submeter sozinho. Mesma regra do pi_models._clean.
        if not alvo or any(c.isspace() for c in alvo) or any(ord(c) < 32 for c in alvo):
            raise ValueError(f"model invalido: {model_id!r}")
        with _send_lock(name):
            self._require_drivable(name)
            send_keys(name, f"/model {alvo}", literal=True)
            time.sleep(_SETTLE)
            send_keys(name, "Enter")
            # SONDA ate a linha de resultado aparecer, em vez de uma foto unica depois de
            # _OPEN_SETTLE: medido ao vivo, o `⎿ Set model to …` demora mais que 0.7s pra ser
            # desenhado e a foto unica reportava "sem confirmacao" numa troca que TINHA dado certo.
            # Quem termina rapido sai na primeira leitura; so quem falhou paga o prazo inteiro.
            fim = time.monotonic() + self._RESULT_PRAZO
            while True:
                time.sleep(_SETTLE)
                pane = tmux.capture_pane(name)
                if mp.picker_open(pane):
                    # O argumento nao foi aceito e o `/model` abriu o picker interativo: fecha e
                    # falha, em vez de deixar um overlay preso e reportar sucesso sobre um no-op.
                    self._abort(name)
                    raise mp.PickerError(409, f"o Claude Code nao aceitou `/model {alvo}`")
                # Compara o TOKEN do modelo, nao substring: a linha da troca anterior continua na
                # tela, e `alvo in result` casava nela quando um id e prefixo do outro — no catalogo
                # da Kimi, ir de `k3-256k` pra `k3` dava sucesso lendo a confirmacao velha, antes de
                # a troca acontecer. Medido: a linha nova demora ~1s a aparecer, entao essa leitura
                # precoce e a regra, nao a exceção.
                if mp.result_model(mp.parse_result_line(pane)) == alvo:
                    return {"ok": True, "result": mp.parse_result_line(pane)}
                if time.monotonic() >= fim:
                    raise mp.PickerError(409, f"sem confirmacao do `/model {alvo}` no terminal")

    def set_model_effort(
        self,
        name: str,
        model: str | None = None,
        effort: str | None = None,
        scope: str = "session",
    ) -> dict:
        """Aplica modelo e/ou esforco dirigindo o picker interativo do `/model`.

        scope='session' aperta `s` (so a sessao atual); scope='default' aperta Enter (salva
        como default). Le o pane a cada passo (drive nao-cego). Em qualquer falha de parse,
        manda Esc e levanta PickerError pra o picker nunca ficar preso.
        """
        if scope not in ("session", "default"):
            raise ValueError("scope must be 'session' or 'default'")
        model_kw = model.strip().lower() if model else None
        effort_kw = effort.strip().lower() if effort else None
        if model_kw is None and effort_kw is None:
            raise ValueError("must provide model or effort")
        # Sem lista chumbada de modelos aqui: quem diz o que existe e o picker lido ao vivo
        # (model_nav_steps levanta se a keyword nao estiver la). Um `MODEL_ORDER` como gate
        # rejeitava, com 422, um modelo NOVO que o picker ja oferecia — foi o que escondeu o Fable.
        # Alnum + o sufixo `[1m]`, que é a notação do próprio Claude Code pra janela de contexto e
        # o que distingue as DUAS linhas `opus` do picker. Sem abrir pro colchete, escolher
        # "Opus (1M context)" voltava 422 depois que o id passou a ser único.
        if model_kw and not _MODEL_KW_OK.match(model_kw):
            raise ValueError(f"unknown model {model!r}")
        if effort_kw and effort_kw not in mp.EFFORT_ORDER:
            raise ValueError(f"unknown effort {effort!r}")
        # Mesmo lock por sessao do send_prompt/send_pi_commands: duas threads digitando no MESMO
        # tty intercalam teclas. Sem ele, a folha buscando a lista (`list_model_options`) enquanto
        # um POST de troca roda dava dois `/model` concorrentes — o `Esc` de um fechava o picker que
        # o outro estava navegando. O `_require_drivable` nao cobre isso: ele ve spinner, nao ve
        # outro driver em voo. A validacao acima fica FORA do lock (nao toca no terminal).
        with _send_lock(name):
            return self._drive_model_effort(name, model_kw, effort_kw, scope)

    def _drive_model_effort(self, name: str, model_kw: str | None, effort_kw: str | None,
                            scope: str) -> dict:
        # 1. Abre o picker.
        pane = self._open_model_picker(name)

        # 2. Navega o modelo (Up/Down). O cursor abre sobre o modelo atual; calculamos os
        #    passos a partir do pane limpo do open (evita reler o cursor, que pode fantasmar
        #    no redraw). Numeros so com setas -- nunca teclas de numero (= confirma default).
        if model_kw:
            rows = mp.parse_model_rows(pane)
            try:
                steps = mp.model_nav_steps(rows, model_kw)
            except ValueError as e:
                self._abort(name)
                raise mp.PickerError(409, f"cannot navigate model picker: {e}")
            key = "Down" if steps > 0 else "Up"
            for _ in range(abs(steps)):
                send_keys(name, key)
                time.sleep(_NAV_GAP)
            time.sleep(_SETTLE)
            pane = tmux.capture_pane(name)

        # 3. Ajusta o esforco (Left/Right, ciclico). Le o marcador a cada Right e para no
        #    alvo; se der a volta inteira sem casar, o nivel nao existe pra esse modelo ->
        #    fica no atual (sem mexer) e segue (o modelo ja e o que importa).
        if effort_kw:
            current = mp.parse_current_effort(pane)
            if current is not None and current != effort_kw:
                start = current
                for _ in range(len(mp.EFFORT_ORDER) + 1):
                    send_keys(name, "Right")
                    time.sleep(_SETTLE)
                    pane = tmux.capture_pane(name)
                    current = mp.parse_current_effort(pane)
                    if current is None:  # redraw transiente -> tenta reler uma vez
                        time.sleep(_SETTLE)
                        pane = tmux.capture_pane(name)
                        current = mp.parse_current_effort(pane)
                    if current == effort_kw:
                        break
                    if current == start:  # ciclo completo: nivel indisponivel pra esse modelo
                        break

        # 4. Confirma: `s` = so a sessao; Enter = salva como default.
        send_keys(name, "s" if scope == "session" else "Enter")
        time.sleep(_OPEN_SETTLE)
        pane = tmux.capture_pane(name)

        # 4b. Trocar o effort pode disparar um follow-up CONDICIONAL "Change effort level?" (so
        #     quando ha cache a re-ler). Por design NAO confirmamos sozinhos: deixamos o menu pra
        #     o usuario decidir (o app o mostra como OptionButtons via state.classify). Reporta
        #     pending_confirm em vez de mascarar como ok aplicado -- o effort so pega quando o
        #     usuario tocar "Yes". (O picker ja fechou; nao mexer no menu.)
        if mp.effort_confirm_open(pane):
            return {"ok": True, "scope": scope, "pending_confirm": effort_kw, "result": None}

        if mp.picker_open(pane):
            self._abort(name)
            raise mp.PickerError(409, "model picker did not close after confirm")

        # 5. Verifica o resultado. Se pedimos sessao mas veio "default", a confirmacao errou
        #    -> expoe a falha em vez de mascarar. (O alvo pode disparar um "Switch model?" de
        #    follow-up; nesse caso nao ha linha de resultado ainda -- ok, o SSE cuida disso.)
        result = mp.parse_result_line(pane)
        if (
            scope == "session"
            and result
            and "session only" not in result.lower()
            and "default" in result.lower()
        ):
            raise mp.PickerError(409, f"expected session-only switch, got: {result}")
        return {"ok": True, "scope": scope, "result": result}

    def send_pi_commands(self, name: str, commands: list[str]) -> None:
        """Digita comandos da extensao do Pi (`/cp-model …`, `/cp-think …`) no pane, em ordem.

        Nao ha picker a dirigir aqui: o Pi aplica a troca pela API de extensao (ver `pi_models`),
        entao isto e so o mesmo maquinario de ENVIO do send_prompt — mesmo lock por sessao (dois
        toques simultaneos nao intercalam teclas no tty), mesmo gate de overlay e mesma espera de
        TUI pronta com o marcador do Pi.

        UM Enter por comando (medido: os nossos comandos nao registram completions de argumento,
        entao o menu de autocomplete fecha no espaco e nao engole o Enter — ao contrario do
        `/model` NATIVO do Pi, que tem completions e reescreve o argumento).
        """
        with _send_lock(name):
            if not deliverable(name):
                raise DriveError("pane com overlay aberto ou sessao morta — nada foi digitado")
            _wait_input_ready(name, provider="pi")
            # Mesma guarda anti-colagem do send_prompt: comando digitado em cima de residuo deixa
            # de ser comando e vira mensagem pro modelo (pior que a colagem de prompt). Achado da
            # review de 31/07 — o gate precisa valer em TODO caminho que digita no composer do Pi.
            if _composer_ocupado_pi(name):
                raise DriveError("composer do pi ja tem texto — comando nao digitado (residuo/rascunho)")
            for cmd in commands:
                send_keys(name, cmd, literal=True)
                time.sleep(_SLASH_SETTLE)
                send_keys(name, "Enter")
                time.sleep(_OPEN_SETTLE)

    def set_kimi_model(self, name: str, alias: str | None = None, display: str | None = None,
                       effort: str | None = None) -> dict:
        """Troca modelo e/ou nível de pensamento de uma sessao Kimi dirigindo o picker do `/model`.

        Sequencia medida ao vivo (Kimi Code 0.37.2, ver kimi_models.py): abre com `/model`+Enter.
        Modelo: digita o ALIAS completo na busca (ela casa alias, nao so nome — a lista do picker e
        invisivel pro capture-pane, entao navegar por setas seria as cegas). Esforço: a linha
        "Thinking (←→ to switch)" mostra os níveis do modelo com o ATUAL colchetado — lê o pane,
        anda Left/Right a diferença de posições. SEMPRE aplica com Alt+S, que troca SO na sessao;
        NUNCA Enter: ele gravaria o alias como default_model GLOBAL no config.toml (medido: o
        arquivo e reescrito na hora). Confirma pela linha NOVA no scrollback — "Switched to …" pra
        modelo, "Thinking set to …" pra esforço — com baseline lida ANTES de digitar (a linha da
        troca anterior continua na tela, mesmo cuidado do set_engine_model com k3-256k -> k3).
        """
        with _send_lock(name):
            if not deliverable(name):
                raise DriveError("pane com overlay aberto ou sessao morta — nada foi digitado")
            _wait_input_ready(name, provider="kimi")
            pane0 = tmux.capture_pane(name) or ""
            antes_model = kimi_models.parse_switched(pane0)
            antes_effort = kimi_models.parse_thinking_set(pane0)
            send_keys(name, "/model", literal=True)
            time.sleep(_SLASH_SETTLE)
            send_keys(name, "Enter")
            # SONDAGEM, não foto única: numa sessão recém-aberta o picker leva ~2s pra pintar a
            # primeira vez (medido no e2e), e checar uma vez só aos 0.7s abortava com o picker
            # abrindo logo depois — o Esc de saída caía ANTES dele e ficava tudo aberto. Sem 2o
            # Enter de insistência (o que o Claude faz em _open_model_picker): aqui ele cairia
            # num picker JÁ aberto meio pintado e confirmaria a linha sob o cursor COMO DEFAULT
            # GLOBAL — o lado errado da falha. Não abriu no prazo: erro e o usuário tenta de novo.
            fim = time.monotonic() + self._RESULT_PRAZO
            aberto = False
            while time.monotonic() < fim:
                time.sleep(_SETTLE)
                if "Select a model" in (tmux.capture_pane(name) or ""):
                    aberto = True
                    break
            if not aberto:
                self._abort(name)
                raise mp.PickerError(409, "o picker do /model do Kimi nao abriu")
            if alias:
                send_keys(name, alias, literal=True)
                # A busca filtra no redraw; teclar o Alt+S em cima da digitacao aplicaria o item que
                # estava sob o cursor ANTES do filtro — o K3 errado com folga.
                time.sleep(_OPEN_SETTLE)
            if effort:
                row = kimi_models.parse_thinking_row(tmux.capture_pane(name) or "")
                if row is None:
                    self._abort(name)
                    raise mp.PickerError(409, "este modelo nao tem niveis de pensamento no picker")
                niveis = [n.lower() for n in row["levels"]]
                if effort not in niveis:
                    self._abort(name)
                    raise mp.PickerError(409, f"nivel {effort} nao aparece no picker deste modelo")
                passos = niveis.index(effort) - niveis.index(row["current"].lower())
                for _ in range(abs(passos)):
                    send_keys(name, "Right" if passos > 0 else "Left")
                    time.sleep(_NAV_GAP)
                time.sleep(_SETTLE)
            send_keys(name, "M-s")
            fim = time.monotonic() + self._RESULT_PRAZO
            while True:
                time.sleep(_SETTLE)
                pane = tmux.capture_pane(name) or ""
                troca = kimi_models.parse_switched(pane)
                pensou = kimi_models.parse_thinking_set(pane)
                # Modelo: "Switched to X with thinking Y". Esforço sozinho: "Thinking set to Y".
                # Pedidos juntos: a linha "Switched" carrega o nível e serve de prova pros dois.
                ok_model = not alias or (
                    troca is not None and troca != antes_model and troca.get("name") == display)
                ok_effort = not effort or (
                    (pensou is not None and pensou != antes_effort and pensou.get("level") == effort)
                    or (troca is not None and troca != antes_model
                        and troca.get("level") == effort))
                if ok_model and ok_effort:
                    linha = (troca or pensou or {}).get("raw") if (troca or pensou) else None
                    return {"ok": True, "result": linha}
                novo = (troca if troca != antes_model else None) or \
                       (pensou if pensou != antes_effort else None)
                if novo is not None:
                    # Saiu uma linha NOVA mas não é o pedido: a busca/navegação casou algo
                    # inesperado. Falha alta, nunca sucesso sobre o que ficou.
                    self._abort(name)
                    raise mp.PickerError(
                        409, f"o Kimi aplicou outra coisa ({novo.get('raw')})")
                if time.monotonic() >= fim:
                    self._abort(name)
                    raise mp.PickerError(409, "sem confirmacao da troca no terminal")

    def _abort(self, name: str) -> None:
        send_keys(name, "Escape")

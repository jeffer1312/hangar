import logging
import re
import threading
import time

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
_READY_MARKERS_BY_PROVIDER = {"pi": ("─", "━", "═", "╰", "│")}

# Timeout por provider. O Pi ficou mais curto de propósito: o boot medido até o composer é ~4.3s,
# então 8s é ~2× de folga, e no estouro a gente ENVIA mesmo assim — ou seja, a espera só compra
# segurança durante o boot e todo o resto é latência pura no dia em que o marcador desandar de novo.
_TIMEOUTS_BY_PROVIDER = {"pi": 8.0}
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


# Numeros dos placeholders de paste ("[Pasted text #N +X lines]") numa regiao do pane. A IDENTIDADE
# importa: aceitar qualquer placeholder fazia um paste ALHEIO (rascunho do usuario) contar como a
# nossa entrega — o Enter submetia o texto do usuario como se fosse o prompt do agente (achado
# CRITICO da review de 31/07). So um numero NOVO em relacao a foto tirada ANTES do nosso paste
# prova alguma coisa.
_PASTE_ID_RE = re.compile(r"\[Pasted text #(\d+)")


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
            # pagar mais um `tmux list-panes` aqui nao pesa como pesaria no /input).
            pane_id = next((p["pane_id"] for p in tmux.list_panes_active() if p["name"] == name), None)
            result = ti.send_prompt(name, entry["text"], provider, pane_id=pane_id)
        except Exception:
            # Falha POS-gate (tty caiu no meio): pode ter emitido tecla -> at-most-once, NAO reverte.
            # ponytail: stranded-mas-visivel (a bubble queued- segue aparecendo, display ignora delivered);
            # upgrade: render distinto / re-drain confirmado-por-transcript se virar reclamacao real.
            return sent
        if result == "partial":
            # Texto ficou pela metade no composer e o Enter NAO foi enviado (fatiamento do Windows).
            # NAO reverte pra delivered=False: o drain reentraria e digitaria o texto inteiro EM CIMA do
            # residuo (nada limpa a linha entre tentativas) — concatenacao, pior que a perda. Fica
            # delivered=True e PARA, com log de erro: a bubble segue visivel no app (o display ignora
            # delivered) e o residuo esta a vista no terminal, entao o usuario ve as duas metades e
            # decide. Silencio aqui era o furo do `except Exception` cego acima.
            _log.error("drain name=%s: envio PARCIAL da entrada %s — parado, sem retry automatico "
                       "(texto pela metade no composer)", name, entry.get("id"))
            return sent
        if result == "deferred":
            # send_prompt NAO tocou a TUI (overlay reabriu entre claim e envio): reverte (provadamente
            # pre-envio) e para — espera o proximo idle. Revert pode falhar (disco): nesse caso a entrada
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


class TerminalInput:
    def send_prompt(self, name: str, text: str, provider: str = "claude",
                    pane_id: str | None = None) -> str:
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
            if provider == "pi" and pane_id and pi_inbox.INBOX.tem_linha(pane_id):
                r = pi_inbox.INBOX.entregar_sync(pane_id, text)
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
                pastes_antes = _paste_ids(_composer_regiao(_capture(name), name) or "")
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
                    _log.error("envio PARCIAL name=%s: multi-linha PAROU no meio da digitacao "
                               "(falha confirmada em tmux.paste_text) — Enter nao enviado — %s",
                               name, _diag_composer(_capture(name), text, name, pastes_antes))
                    return "partial"
                # Settle ANTES do Enter, como no ramo de uma linha. Ver _MULTILINE_SUBMIT_SETTLE:
                # os 0.05 antigos eram menores que a ingestao MINIMA medida (0.08s) e o Enter
                # submetia o texto pela metade.
                time.sleep(_MULTILINE_SUBMIT_SETTLE)
                if not _entrou_no_composer(name, text, pastes_antes):
                    # NAO aperta Enter: o texto nao chegou no composer, entao o Enter submeteria o que
                    # estivesse la (a primeira linha truncada, ou nada) como se fosse pedido do usuario.
                    _log.error("envio PARCIAL name=%s: multi-linha NAO chegou no composer em %.1fs "
                               "(o multiplexador aceitou e nao entregou) — Enter nao enviado — %s",
                               name, _SUBMIT_CHECK_PRAZO, _diag_composer(_capture(name), text, name, pastes_antes))
                    return "partial"
                send_keys(name, "Enter")
                # CONFERE em vez de confiar no settle. Caso real medido: tres recados longos
                # cross-server sairam com delivered=True e NUNCA viraram entrada no transcript do
                # destino — ficaram com attempts=2 na fila (requeue duas vezes e desistencia), e o
                # dono do outro lado so os achou lendo o sidecar. Um settle maior reduz a chance e nao
                # detecta nada: o Enter correndo a ingestao devolve "sent" do mesmo jeito.
                if not _submeteu(name, text, pastes_antes):
                    _log.error("envio PARCIAL name=%s: multi-linha nao submeteu (a cauda do texto "
                               "continua no composer apos %.1fs) — nao afirmando entrega — %s",
                               name, _SUBMIT_CHECK_PRAZO, _diag_composer(_capture(name), text, name, pastes_antes))
                    return "partial"
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
                    # entrega. O texto parcial FICA visível no composer — nada aqui limpa a linha, e
                    # limpar às cegas exigiria saber qual tecla zera o composer em cada TUI.
                    # ponytail: sem limpeza automática; upgrade = medir a tecla de limpar (C-u/Esc) por
                    # provider e zerar antes de devolver, pra um retry não digitar em cima do resíduo.
                    _log.error("envio PARCIAL name=%s: texto ficou pela metade no input, Enter NAO enviado",
                               name)
                    return "partial"
                # Settle ANTES do Enter: sem isto o Enter corria a ingestao do texto e o claude (que
                # detecta input rapido como paste) tratava o Enter como parte do conteudo -> o texto
                # ficava no input SEM submeter (usuario tinha que reenviar). Espelha o gap multiline.
                # ponytail: settle fixo; se ainda escapar em device lento, upgrade = capturar o pane e
                # reenviar Enter se o input nao limpou.
                time.sleep(_SUBMIT_SETTLE)
                if not _entrou_no_composer(name, text, pastes_antes):
                    _log.error("envio PARCIAL name=%s: o texto NAO chegou no composer em %.1fs — "
                               "Enter nao enviado — %s", name, _SUBMIT_CHECK_PRAZO,
                               _diag_composer(_capture(name), text, name, pastes_antes))
                    return "partial"
                send_keys(name, "Enter")
                # Mesma conferencia do ramo multi-linha: e o upgrade que o comentario acima ja anotava
                # ("capturar o pane e reenviar Enter se o input nao limpou"). Aqui em vez de reenviar
                # Enter as cegas a gente REPORTA — reenviar podia submeter texto que o usuario digitou
                # no composer no meio do caminho.
                if not _submeteu(name, text, pastes_antes):
                    _log.error("envio PARCIAL name=%s: uma linha nao submeteu (texto continua no "
                               "composer apos %.1fs) — nao afirmando entrega — %s", name, _SUBMIT_CHECK_PRAZO,
                               _diag_composer(_capture(name), text, name, pastes_antes))
                    return "partial"
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
        if model_kw and (not model_kw.isascii() or not model_kw.isalnum()):
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

    def _abort(self, name: str) -> None:
        send_keys(name, "Escape")

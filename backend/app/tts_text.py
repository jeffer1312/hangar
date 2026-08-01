import re

# Preparo do texto que vai virar audio.
#
# Pequeno de proposito: quem manda o texto e o FRONT, e la ele ja sai do DOM renderizado (o
# renderMarkdown ja consumiu ##, ** e backtick) com os blocos de codigo trocados pelo marcador. O que
# sobra pra ca e o que o navegador nao tem como resolver.
#
# Verbalizar numero e data NAO entra aqui: e o que o apply_text_normalization da ElevenLabs faz, e
# ele vai explicito no corpo da requisicao (ver tts.py).

# Caminho de arquivo: "frontend/src/lib/api.ts" soletrado inteiro e insuportavel de ouvir.
_CAMINHO = re.compile(r"\b(?:[\w.-]+/){1,}([\w-]+)\.(\w{1,5})\b")

# Hash (commit, sha, id): "8f94525..2e70e70" soletrado letra a letra e a coisa mais inutil que a voz
# pode fazer — ninguem decora hash de ouvido, e no meio de uma frase ele vira ruido puro. Some, e o
# resto da frase ("No ar: ... Recarrega") continua fazendo sentido.
#
# Exige pelo menos um DIGITO e pelo menos uma LETRA a-f. As duas condicoes existem por um falso
# positivo cada, os dois medidos:
#   - so digito seria "1234567", um numero de verdade que a pessoa quer ouvir;
#   - so letra seria "acceded"/"defaced"/"efface", palavra normal composta so de letras hex.
# E 7+ caracteres pra nao pegar "cafe" nem "add".
_HASH = re.compile(r"\b(?=[0-9a-f]*\d)(?=[0-9a-f]*[a-f])[0-9a-f]{7,40}\b")
_SETA = re.compile(r"[←-⇿⬀-⯿]")
_EMOJI = re.compile(r"[☀-➿️\U0001F000-\U0001FAFF]")
_BULLET = re.compile(r"[•·▪◦‣●○◆■]")


def preparar(texto: str) -> str:
    """Texto falavel a partir do texto ja achatado pelo front. Devolve '' quando nao sobra fala —
    o chamador usa isso pra recusar em vez de sintetizar silencio pago."""
    t = _CAMINHO.sub(lambda m: f"{m.group(1)} ponto {m.group(2)}", texto)
    # Antes do "_ vira espaco": hash nao tem underline, e tirar cedo evita que um id com underline
    # vire duas palavras e escape do casamento.
    t = _HASH.sub(" ", t)
    t = t.replace("..", " ")   # sobra do intervalo de commits ("a..b"), agora sem as pontas
    t = t.replace("_", " ")
    t = _SETA.sub(" ", t)
    t = _EMOJI.sub(" ", t)
    t = _BULLET.sub(" ", t)
    t = t.replace("—", ", ").replace("–", ", ")   # travessao vira pausa
    # \n+ (nao so \n{2,}): o front agora manda UMA quebra por bloco (ver speakable.ts), entao um
    # titulo seguido de um paragrafo ja chega como uma unica \n — sem isso essa pausa era
    # inalcancavel, porque so quebra DUPLA virava pausa. Duas etapas, a especifica antes da geral:
    # quem ja termina em pontuacao de frase (inclui o marcador de codigo omitido) so precisa da
    # quebra virar espaco — virar ". " tambem duplicaria o ponto ("arquivo.." num plano, onde quase
    # toda linha ja termina em pontuacao). Titulo/item de lista sem pontuacao final ganham a pausa.
    t = re.sub(r"([.!?:;…])[ \t]*\n+", r"\1 ", t)
    t = re.sub(r"\n+", ". ", t)
    t = re.sub(r"\s+", " ", t)
    return t.strip()

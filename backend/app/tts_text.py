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
_SETA = re.compile(r"[←-⇿⬀-⯿]")
_EMOJI = re.compile(r"[☀-➿️\U0001F000-\U0001FAFF]")
_BULLET = re.compile(r"[•·▪◦‣●○◆■]")


def preparar(texto: str) -> str:
    """Texto falavel a partir do texto ja achatado pelo front. Devolve '' quando nao sobra fala —
    o chamador usa isso pra recusar em vez de sintetizar silencio pago."""
    t = _CAMINHO.sub(lambda m: f"{m.group(1)} ponto {m.group(2)}", texto)
    t = t.replace("_", " ")
    t = _SETA.sub(" ", t)
    t = _EMOJI.sub(" ", t)
    t = _BULLET.sub(" ", t)
    t = t.replace("—", ", ").replace("–", ", ")   # travessao vira pausa
    # \n+ (nao so \n{2,}): o front agora manda UMA quebra por bloco (ver speakable.ts), entao um
    # titulo seguido de um paragrafo ja chega como uma unica \n — sem isso essa pausa era
    # inalcancavel, porque so quebra DUPLA virava pausa.
    t = re.sub(r"\n+", ". ", t)
    t = re.sub(r"\s+", " ", t)
    return t.strip()

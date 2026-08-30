"""Traduz o resumo do pensamento pro português, sob demanda.

Por que existe: o resumo vem da API da Anthropic, e ela o escreve quase sempre em inglês — medido
em 29/08/2026, inclusive com a conversa inteira em pt-BR e com o modelo instruído a pensar em
português (o sumarizador é outro, do lado deles, e não obedece o prompt da conversa). Também vem
mais verboso que o do app deles: 190 a 360 caracteres contra os títulos de uma linha do iPhone.

Duas decisões que valem regra:

* **Só quando ABRE.** A tradução é disparada pelo front no primeiro clique no bloco, e nunca no
  parse. Traduzir tudo na chegada gastaria uma chamada por pensamento — 4333 deles só nos
  transcripts desta máquina — pra um texto que quase ninguém abre.
* **Falhar é devolver o original.** Provedor sem chave, fora do ar ou lento não pode custar o
  conteúdo: quem chama recebe o inglês de volta e a tela não muda. É por isso que o endpoint
  responde 200 com o texto cru em vez de erro.
"""
import hashlib
import logging
import threading
import time

from app.narrar import NarrarError, chamar_chat

_log = logging.getLogger("hangar.pensamento_pt")

# Curto de propósito: o alvo é o bullet do app do iPhone (8 a 12 palavras), não a tradução fiel do
# parágrafo. "Não invente" está aqui porque encurtar é justamente onde um modelo preenche buraco.
_SYSTEM = (
    "Você reescreve, em português do Brasil, o resumo do raciocínio de um assistente de "
    "programação. Regras: uma frase curta por passo, no gerúndio ou no infinitivo "
    "('Conferindo se…', 'Buscando…'); no máximo 14 palavras por passo; preserve nomes próprios, "
    "comandos, caminhos e URLs exatamente como estão; não invente nada que não esteja no texto; "
    "não comente, não explique, não use aspas. Responda só com o texto reescrito, mantendo as "
    "quebras de parágrafo do original."
)

_TIMEOUT = 12
_TEMPERATURA = 0.2
# O bloco manda todos os seus pensamentos de uma vez e as chamadas são SEQUENCIAIS; o navegador
# desiste em 30s. Sem teto total, um bloco de 20 textos podia segurar a thread por 4 minutos
# traduzindo texto que ninguém mais ia receber. Estourado o prazo, o resto volta como veio.
_PRAZO_TOTAL = 22.0
# Texto acima disto não é resumo, é outra coisa (o Pi e o Kimi mandam raciocínio CRU aqui, que não
# tem tamanho previsível). Traduzir não ajudaria a ler, e mandaria um corpo enorme pro provedor.
MAX_CHARS = 4000
# Cache em memória: o mesmo bloco é reaberto e a lista re-renderiza várias vezes por sessão. Some
# no restart de propósito — é conveniência, não dado; guardar em disco pediria invalidação e uma
# pasta nova pra economizar uma chamada de 1,2s.
_MAX_CACHE = 500
_cache: dict[str, str] = {}
_lock = threading.Lock()


def _chave(texto: str) -> str:
    return hashlib.sha256(texto.encode("utf-8")).hexdigest()


def traduzir(texto: str) -> str:
    """Texto em pt-BR, curto. Devolve o ORIGINAL em qualquer falha (nunca levanta)."""
    texto = texto.strip()
    if not texto or len(texto) > MAX_CHARS:
        return texto
    k = _chave(texto)
    with _lock:
        pronto = _cache.get(k)
    if pronto is not None:
        return pronto
    try:
        saida = chamar_chat(_SYSTEM, texto, temperature=_TEMPERATURA, timeout=_TIMEOUT).strip()
    except NarrarError as e:
        _log.info("traducao do pensamento falhou (%s): %s", e.status, e.detail)
        return texto
    except Exception:                       # noqa: BLE001 — tradução nunca derruba a leitura
        _log.warning("traducao do pensamento falhou", exc_info=True)
        return texto
    if not saida:
        return texto
    with _lock:
        if len(_cache) >= _MAX_CACHE:
            _cache.clear()                  # ponytail: limpa tudo em vez de LRU; é cache de texto curto
        _cache[k] = saida
    return saida


def traduzir_varios(textos: list[str]) -> list[str]:
    """Traduz em ordem, PARANDO no prazo total. O que sobrar volta como veio.

    O prazo existe porque as chamadas são sequenciais e o navegador desiste antes: gastar minutos
    do provedor produzindo texto que ninguém vai receber é custo puro. Já traduzido fica no cache,
    então a próxima passada continua de onde parou em vez de recomeçar.
    """
    prazo = time.monotonic() + _PRAZO_TOTAL
    saida = []
    for t in textos:
        saida.append(traduzir(t) if time.monotonic() < prazo else t)
    return saida

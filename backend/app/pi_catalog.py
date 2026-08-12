"""Catálogo de modelos do Pi: fonte única da LISTA.

`pi --list-models` roda solto no terminal, sem sessão viva — é o que a tela de abertura precisa. E
medido em 10/08/2026, ele traz exatamente os mesmos 384 modelos que o sidecar da extensão, com dois
campos a mais (contexto e imagem). Por isso a folha da sessão viva também usa esta fonte: uma lista
só, com etiqueta, nos dois lugares.

O sidecar continua indispensável pro que só ele sabe: modelo atual, nível atual e `levels` — quais
níveis AQUELE modelo aceita, que variam por modelo e não aparecem aqui.

Cache porque é subprocess Node: pagar isso a cada abertura de tela é caro pra uma lista que muda de
mês em mês.
"""
import subprocess
import time

_TTL = 600.0
_cache: tuple[float, list[dict]] | None = None


def parse(saida: str) -> list[dict]:
    out: list[dict] = []
    for linha in saida.splitlines():
        campos = linha.split()
        # 6 colunas exatas; o cabeçalho cai pelo próprio nome, e linha torta é pulada em vez de
        # derrubar a lista — um provedor novo com coluna a mais não pode cegar o seletor.
        if len(campos) != 6 or campos[0] == "provider":
            continue
        provider, ident, context, max_out, thinking, images = campos
        out.append({"provider": provider, "id": ident, "context": context,
                    "max_out": max_out, "thinking": thinking == "yes", "images": images == "yes"})
    return out


def listar(fresco: bool = False) -> list[dict]:
    global _cache
    if _cache and not fresco and time.monotonic() - _cache[0] < _TTL:
        return _cache[1]
    r = subprocess.run(["pi", "--list-models"], capture_output=True, text=True, timeout=30)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip() or "pi --list-models falhou")
    modelos = parse(r.stdout)
    _cache = (time.monotonic(), modelos)
    return modelos

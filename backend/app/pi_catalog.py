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
    # `encoding` explicito pelo mesmo motivo dos outros: `text=True` sozinho decodifica pelo
    # locale, cp1252 no Windows, e a tabela do `pi` traz rotulo de modelo que nao e so ASCII.
    r = subprocess.run(["pi", "--list-models"], capture_output=True, text=True,
                       encoding="utf-8", errors="replace", timeout=30)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip() or "pi --list-models falhou")
    modelos = parse(r.stdout)
    if not modelos:
        # rc=0 com tabela que o parse não reconhece (coluna a mais, saída truncada, saída
        # vazia) é falha do provedor, não catálogo vazio. Levanta pra virar o 502 que a rota
        # já sabe dar, e NÃO cacheia: senão o erro dura 10 min depois de o pi voltar.
        primeira = (r.stdout.strip().splitlines() or [""])[0][:120]
        raise RuntimeError(f"pi --list-models nao devolveu modelo nenhum (1a linha: {primeira!r})")
    # O `errors="replace"` acima e rede contra byte que nao decodifica (sem ele, medido no
    # Windows, o erro morre DENTRO da thread leitora do subprocess, o `run()` nao levanta nada e o
    # `stdout` volta None — o TypeError cai longe da causa). O preco e o byte ruim virar `�` e
    # seguir como texto. Aqui isso importa mais que num rotulo: o `id` e DIGITADO na TUI depois
    # (`/cp-model <provider> <id>`), entao um `�` no meio dele vira uma troca de modelo que falha
    # sem ninguem entender por que. Mesma doutrina do bloco acima — tabela em que nao da pra
    # confiar e falha do provedor: 502 e NAO cacheia. Rotulo ilegivel em coluna que so se le
    # (contexto, max_out) passa: ali `�` e feio, nao errado.
    ruins = [m["id"] for m in modelos if "�" in m["id"] or "�" in m["provider"]]
    if ruins:
        raise RuntimeError(
            f"pi --list-models devolveu {len(ruins)} id/provider com byte ilegivel (ex: {ruins[:3]})")
    _cache = (time.monotonic(), modelos)
    return modelos

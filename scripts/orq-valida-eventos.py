#!/usr/bin/env python3
"""Valida um eventos.jsonl contra o contrato da skill (arbitro.md).
Uso: python3 scripts/orq-valida-eventos.py <arquivo> [...]
Sai 0 se todos válidos; imprime linha e defeito de cada inválida."""
import json
import sys

TIPOS = {
    "execucao_inicio": {"plano", "branch", "gid"},
    "task_inicio": {"task", "titulo", "executor", "par"},
    "entrega": {"task", "rodada"},
    "veredito": {"task", "rodada", "resultado", "sessao"},
    "sessao_trocada": {"de", "para"},
    "execucao_fim": {"resultado"},
}
RESULTADOS = {"aprova", "reprova", "devolvido"}


def valida(path: str) -> int:
    erros = 0
    # `errors="replace"`: a ferramenta existe pra diagnosticar arquivo quebrado — morrer com
    # traceback num byte inválido seria justo o caso que ela deveria reportar. O byte trocado cai
    # no json.loads da linha e sai como "json invalido", com o número da linha.
    with open(path, encoding="utf-8", errors="replace") as f:
        return _valida_linhas(path, f)


def _valida_linhas(path: str, linhas) -> int:
    erros = 0
    for i, linha in enumerate(linhas, 1):
        linha = linha.strip()
        if not linha:
            continue
        try:
            ev = json.loads(linha)
        except ValueError as e:
            print(f"{path}:{i}: json invalido — {e}"); erros += 1; continue
        tipo = ev.get("tipo")
        if tipo not in TIPOS:
            print(f"{path}:{i}: tipo desconhecido {tipo!r}"); erros += 1; continue
        if "ts" not in ev:
            print(f"{path}:{i}: sem ts"); erros += 1
        faltam = TIPOS[tipo] - ev.keys()
        if faltam:
            print(f"{path}:{i}: {tipo} sem {sorted(faltam)}"); erros += 1
        for campo in ("task", "rodada"):
            # `bool` é subclasse de int: sem a exclusão, `"rodada": true` passava aqui e o parser
            # real (orq._int_ou_none) o descartava como rodada desconhecida — validador dando
            # confiança falsa justo no campo que alimenta o KPI de "aprovada de primeira".
            if (campo in ev and campo in TIPOS[tipo]
                    and (isinstance(ev[campo], bool) or not isinstance(ev[campo], int))):
                print(f"{path}:{i}: {campo} nao e numero ({ev[campo]!r})"); erros += 1
        if tipo == "veredito" and ev.get("resultado") not in RESULTADOS:
            print(f"{path}:{i}: resultado {ev.get('resultado')!r} fora de {sorted(RESULTADOS)}"); erros += 1
    return erros


if __name__ == "__main__":
    total = sum(valida(p) for p in sys.argv[1:])
    sys.exit(1 if total else 0)

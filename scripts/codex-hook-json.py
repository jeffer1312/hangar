#!/usr/bin/env python3
"""Adapta a telemetria do security-guidance ao JSON estrito dos hooks do Codex."""
import json
import subprocess
import sys


def normalizar(bruto: bytes) -> bytes:
    try:
        try:
            dados = [json.loads(bruto)]
        except ValueError:
            dados = [json.loads(linha) for linha in bruto.splitlines() if linha.strip()]
        if not dados or any(not isinstance(d, dict) for d in dados):
            return bruto
        # O bootstrap anuncia async antes da resposta. O Codex aguarda o processo inteiro.
        assincrono = False
        if (len(dados) == 2 and dados[0].get("async") is True
                and set(dados[0]) <= {"async", "asyncTimeout"}):
            dados.pop(0)
            assincrono = True
        if len(dados) != 1:
            return bruto
        saida = dados[0].copy()
        saida.pop("metrics", None)
        resumo = saida.pop("rewakeSummary", None)
        if isinstance(resumo, str) and resumo and not saida.get("systemMessage"):
            saida["systemMessage"] = resumo
        if saida == dados[0] and not assincrono:
            return bruto
        return json.dumps(saida, ensure_ascii=True).encode() + b"\n"
    except (ValueError, UnicodeError):
        return bruto


def main() -> int:
    if len(sys.argv) != 3 or sys.argv[1] != "--":
        print("Uso: codex-hook-json.py -- comando", file=sys.stderr)
        return 2
    try:
        r = subprocess.run(sys.argv[2], shell=True, input=sys.stdin.buffer.read(), capture_output=True)
    except OSError as exc:
        print(f"Não foi possível executar o hook: {exc}", file=sys.stderr)
        return 127
    sys.stdout.buffer.write(normalizar(r.stdout))
    sys.stderr.buffer.write(r.stderr)
    # O código de bloqueio pertence ao hook, inclusive quando o JSON exige adaptação.
    return r.returncode if r.returncode >= 0 else 128 - r.returncode


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Completa a saída de um hook PreToolUse pro Codex: `updatedInput` sem `permissionDecision` vira
`allow`. Claude Code aceita a reescrita sem a decisão; o Codex recusa ("PreToolUse hook returned
updatedInput without permissionDecision:allow") e roda o comando original. Qualquer outra saída
(vazia, não-JSON, decisão já dada) passa intacta. stdlib-only: roda no python3 do sistema."""
import json
import subprocess
import sys


def completar_allow(bruto: bytes) -> bytes:
    """Só completa a reescrita de PreToolUse; preserva outras saídas byte a byte."""
    try:
        saida = json.loads(bruto)
    except (ValueError, UnicodeError):
        return bruto
    if not isinstance(saida, dict):
        return bruto
    esp = saida.get("hookSpecificOutput")
    if (isinstance(esp, dict) and esp.get("hookEventName") == "PreToolUse"
            and "updatedInput" in esp and "permissionDecision" not in esp):
        esp["permissionDecision"] = "allow"
        # Escapes JSON também preservam substitutos Unicode isolados recebidos do hook.
        return json.dumps(saida, ensure_ascii=True).encode("utf-8")
    return bruto


def main() -> int:
    bruto = sys.stdin.buffer.read()
    if len(sys.argv) == 1:
        # Compatibilidade com o filtro antigo, usado depois de um pipe.
        sys.stdout.buffer.write(completar_allow(bruto))
        return 0
    if sys.argv[1] != "--" or len(sys.argv) < 3:
        print("Uso: codex-hook-allow.py [-- comando argumentos...]", file=sys.stderr)
        return 2
    try:
        resultado = subprocess.run(sys.argv[2:], input=bruto, capture_output=True)
    except OSError as exc:
        print(f"Não foi possível executar o hook: {exc}", file=sys.stderr)
        return 127
    sys.stderr.buffer.write(resultado.stderr)
    sys.stdout.buffer.write(completar_allow(resultado.stdout))
    # No POSIX, subprocess representa término por sinal com um código negativo.
    return resultado.returncode if resultado.returncode >= 0 else 128 - resultado.returncode


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Completa a saída de um hook PreToolUse pro Codex: `updatedInput` sem `permissionDecision` vira
`allow`. Claude Code aceita a reescrita sem a decisão; o Codex recusa ("PreToolUse hook returned
updatedInput without permissionDecision:allow") e roda o comando original. Qualquer outra saída
(vazia, não-JSON, decisão já dada) passa intacta. stdlib-only: roda no python3 do sistema."""
import json
import sys

bruto = sys.stdin.read()
try:
    saida = json.loads(bruto)
    esp = saida["hookSpecificOutput"]
    if "updatedInput" in esp and "permissionDecision" not in esp:
        esp["permissionDecision"] = "allow"
        bruto = json.dumps(saida, ensure_ascii=False)
except (ValueError, KeyError, TypeError):
    pass
sys.stdout.write(bruto)

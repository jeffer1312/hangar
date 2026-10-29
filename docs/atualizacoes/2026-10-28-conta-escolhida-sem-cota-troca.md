---
id: 2026-10-28-conta-escolhida-sem-cota-troca
titulo: As sessões passam a saber que a conta pedida sem cota também nasce na de mais folga
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send
destrutivo: false
---

As instruções que cada sessão recebe do Hangar agora dizem que uma sessão aberta numa conta com
95% ou mais de uso nasce na conta de mais folga mesmo quando a conta foi pedida de propósito, e não
só quando foi herdada.

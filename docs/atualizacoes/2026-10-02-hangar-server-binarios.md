---
id: 2026-10-02-hangar-server-binarios
titulo: As conversas do Claude e do Codex passam a ser servidas por uma parte nova do servidor, em Rust
comando_posix: ./scripts/install-server-binaries.sh --never-fail
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/install-server-binaries.ps1 -NeverFail
prova: scripts/install-server-binaries.sh scripts/install-server-binaries.ps1
destrutivo: true
---

O histórico e o chat ao vivo das conversas do Claude e do Codex passam a sair de um programa em
Rust que fica na frente do servidor de sempre. Sem ele (máquina sem versão publicada ou download
que falhou), o app continua funcionando como antes.

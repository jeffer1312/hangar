---
id: 2026-10-29-peers-doctor-rust
titulo: Diagnóstico e leitura de máquinas sem o servidor aberto
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: scripts/hangar-server-command scripts/hangar-server-command.ps1
destrutivo: false
---

O diagnóstico da instalação e a consulta às máquinas cadastradas continuam disponíveis com o servidor parado. Os atalhos passam a usar o executável da versão instalada.

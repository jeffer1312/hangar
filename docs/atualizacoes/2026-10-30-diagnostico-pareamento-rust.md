---
id: 2026-10-30-diagnostico-pareamento-rust
titulo: Diagnóstico e pareamento sem Python
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-doctor.cmd
destrutivo: false
---

O diagnóstico e os códigos de pareamento usam o executável nativo, mesmo com o serviço
parado. O lançador do Windows passa a identificar explicitamente a instalação consultada.

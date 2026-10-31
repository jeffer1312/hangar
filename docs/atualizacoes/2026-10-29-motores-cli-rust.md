---
id: 2026-10-29-motores-cli-rust
titulo: Motores usam o executável nativo
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-engine.cmd
destrutivo: false
---

O comando de motores passa a funcionar sem Python e sem o serviço aberto. No Windows,
o lançador instalado é atualizado para chamar o executável nativo.

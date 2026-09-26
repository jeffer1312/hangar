---
id: 2026-09-26-protocolo-sessoes-irmas
titulo: O protocolo das sessões no CLAUDE.md para de mandar usar o SendMessage
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send
destrutivo: false
---

O bloco "Sessões-irmãs" do CLAUDE.md global deixa de mandar cair pro `SendMessage` quando o
aviso de grupo sai com código 3, que não acontece mais: o Hangar entrega a todos. Ele passa a
citar `hangar-send --close` e `hangar-preview objetivo`/`confere`, e o que só existe no CLI.

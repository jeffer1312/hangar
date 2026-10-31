---
id: 2026-10-31-links-do-pacote
titulo: Atalhos, skills e bloco do shell voltam a apontar para a versão instalada
comando_posix: ./scripts/install-claude-wrapper.sh
prova: ~/.local/bin/hangar-codex-tui ~/.local/bin/hangar-send
tipo: maquina
destrutivo: false
---

Em algumas máquinas que passaram para o pacote, os atalhos do Hangar continuavam no código antigo,
e o Codex podia não abrir depois de uma atualização. Agora eles seguem a versão instalada.

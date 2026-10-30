---
id: 2026-10-30-harness-instalacao-nativa
titulo: Instalação e saúde dos agentes no servidor nativo
comando_posix: ./scripts/install-claude-wrapper.sh --no-statusline && ./scripts/install-skills-bridge.sh
prova: scripts/hangar-harness-setup
destrutivo: false
---

O painel instala e confere os agentes pelo servidor nativo. Os comandos locais de instalação
e o espelhamento de skills funcionam sem Python, inclusive com o serviço parado.

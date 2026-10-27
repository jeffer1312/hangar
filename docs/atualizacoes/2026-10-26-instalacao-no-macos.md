---
id: 2026-10-26-instalacao-no-macos
titulo: Wrappers do claude e do codex voltam a instalar no macOS
comando_posix: ./scripts/install-claude-wrapper.sh
prova: ~/.local/bin/hangar-open ~/.local/bin/hangar-conta
destrutivo: false
---

No macOS a instalação dos wrappers parava no meio por causa do bash e do awk que vêm com o
sistema; agora eles instalam. O instalador também passa a exigir o Python 3.10 ou mais novo,
baixa o tmux pronto em vez de compilar e encontra o Tailscale instalado como app.

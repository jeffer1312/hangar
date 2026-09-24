# Pôr o servidor jefferson-felizardo igual ao notebook (app nativo do Hangar)

Mesmo usuário, mesmo CachyOS. Cada passo: conferir primeiro, instalar só o que faltar, relatar o que mudou.

1. Hangar na `main` atual: `cd ~/Projetos/hangar && git status` (árvore suja → pare e avise), `git pull --ff-only`; a `main` do notebook está em `3fdd2abd`. Depois `./scripts/install-hangar-send.sh`, `./scripts/install-claude-wrapper.sh` e `systemctl --user restart hangar-backend.service`.
2. Claude Code: `claude --version` ≥ 2.1.281; senão `claude update`.
3. Rust: `rustup --version`; `rustup toolchain install 1.98.1 --profile minimal`; `rustup component add rust-analyzer --toolchain 1.98.1`; se `rust-analyzer` não estiver no PATH: `ln -s /usr/bin/rustup ~/.local/bin/rust-analyzer` (o rustup vira o proxy).
4. Plugin LSP: `CLAUDE_CONFIG_DIR=$HOME/.claude claude plugin install rust-analyzer-lsp@claude-plugins-official` e, em `~/.claude/settings.json` (o principal, nunca a cópia de uma conta), `"rust-analyzer-lsp@claude-plugins-official": true` em `enabledPlugins`.
5. Skills do gpui-kit (a biblioteca de interface do app): `npx -y skills add longbridge/gpui-kit -g -a claude-code -y -s gpui-kit` e o mesmo com `-s gpui-kit-design-guides`. Confira `~/.claude/skills/gpui-kit/SKILL.md`.
6. ECC com Rust ligado: acrescente `rust-patterns` e `rust-testing` a `~/.claude/ecc-slim-keep.txt`, `rust-reviewer` e `rust-build-resolver` a `~/.claude/ecc-agents-keep.txt`, `rust-build`, `rust-review` e `rust-test` a `~/.claude/ecc-slim-keep-commands.txt` (sem duplicar), depois `bash ~/.claude/bin/ecc-slim.sh`.
7. Código: `git fetch origin native-desktop` e `git worktree add ~/Projetos/hangar-native-desktop native-desktop` (se a pasta já existir, pare e avise).
8. Conferência: `cd ~/Projetos/hangar-native-desktop/desktop-native && cargo build --locked --release` sem erro (dependências do sistema para GPUI: vulkan, wayland, libxkbcommon, fontconfig; instale o que o erro pedir, com pacman).
9. Relatório curto: versões (Claude Code, rustup, rust-analyzer), commit da `main`, branch/HEAD da worktree, resultado do build, o que foi instalado ou mudado.

Não commitar nem dar push; não editar a cópia de settings de conta; não mexer em sessões de outros.

---
id: 2026-09-28-link-hangar-abre-o-app-nativo
titulo: Links hangar:// passam a abrir o app nativo
comando_posix: ./scripts/install-native.sh --forcar
comando_windows: powershell -ExecutionPolicy Bypass -File scripts/install-native.ps1 -Forcar
prova: ~/.hangar/native/scheme-hangar
destrutivo: false
tipo: maquina
---

Clicar num link de convite `hangar://convite/...` agora abre o app nativo já com o convite colado, sem
entrar sozinho: a entrada só acontece no seu clique. Se o app já estiver aberto, é ele que vem para a frente.

No Linux quem registra o link é o instalador que vem dentro do pacote baixado da release: antes de a release
nativa com esta mudança ser publicada, o passo baixa o pacote antigo, não registra nada e volta a tentar na
próxima atualização.

No macOS e em máquina sem versão nativa publicada não há o que registrar: o passo só deixa a marca e não volta a rodar.

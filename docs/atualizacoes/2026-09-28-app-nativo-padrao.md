---
id: 2026-09-28-app-nativo-padrao
titulo: O app nativo passa a ser a janela padrão do Hangar
comando_posix: ./scripts/install-native.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.hangar/native/release.json
destrutivo: false
tipo: maquina
---

O atalho "Hangar" agora abre o app nativo, baixado da release do GitHub no pacote certo para este
sistema e conferido pelo sha256 antes de instalar. Ele já abre conectado a este servidor e se
atualiza sozinho pelo botão do topo. O Electron continua instalado ao lado, como "Hangar
(Electron)". Máquina sem versão nativa publicada (Linux ARM, Mac Intel) segue com o Electron.

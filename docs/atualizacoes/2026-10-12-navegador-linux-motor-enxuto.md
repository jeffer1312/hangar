---
id: 2026-10-12-navegador-linux-motor-enxuto
titulo: No Linux, o navegador do app nativo passa a usar o chrome-headless-shell mesmo com Chrome instalado
comando_posix: ./scripts/install-chromium.sh
prova: ~/.hangar/native/chromium-ok
destrutivo: false
tipo: maquina
---

No Linux, o navegador do painel do app nativo usava o Google Chrome do sistema quando havia um. O
gerenciador de senhas dele prende a página: depois de um login com senha, aquele site para de responder
aos cliques até o navegador ser reaberto. A atualização baixa o `chrome-headless-shell` (cerca de 260 MB
em disco), que passa a ser o padrão. Quem preferir o Chrome completo liga "Usar o Chrome completo do
sistema" em Configurações > Geral.

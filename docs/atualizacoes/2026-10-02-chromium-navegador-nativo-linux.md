---
id: 2026-10-02-chromium-navegador-nativo-linux
titulo: No Linux, o navegador do app nativo passa a ser um Chromium sem janela
comando_posix: ./scripts/install-chromium.sh
prova: ~/.hangar/native/chromium-ok
destrutivo: false
tipo: maquina
---

No Linux, o navegador do painel lateral do app nativo passa a usar um Chromium sem janela, e com
ele o app atende o `hangar-preview` e a tela remota do navegador no celular. Se a máquina já tem o
Google Chrome ou o Chromium, nada é baixado; senão, a atualização baixa o `chrome-headless-shell`
(cerca de 260 MB em disco). A WPE WebKit deixa de ser necessária.

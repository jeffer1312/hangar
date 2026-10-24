---
id: 2026-09-29-tarefas-windows-prioridade-normal
titulo: No Windows, o Hangar e as sessões deixam de rodar com prioridade abaixo do normal
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/AppData/Local/hangar/hangar-backend.vbs
destrutivo: false
tipo: maquina
---

Com a máquina carregada, o Hangar no Windows ficava lento a ponto de a vigia reiniciá-lo e o
celular perder a conexão. Ele e as sessões que abre agora disputam a CPU de igual para igual com
os outros programas.

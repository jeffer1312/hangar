---
id: 2026-10-20-backend-abre-o-proprio-log
titulo: No Windows, um programa que uma sessão deixou aberto não impede mais o backend de voltar
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/AppData/Local/hangar/hangar-backend.vbs
destrutivo: false
tipo: maquina
---

No Windows, quando o backend reiniciava (numa atualização, por exemplo) e uma sessão tinha deixado
um teste, um port-forward ou um navegador aberto, a tarefa do backend não conseguia mais subir e o
servidor ficava fora do ar sem nenhum aviso no log. O backend agora sobe mesmo nesse caso.

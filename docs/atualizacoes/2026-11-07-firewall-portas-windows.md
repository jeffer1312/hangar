---
id: 2026-11-07-firewall-portas-windows
titulo: No Windows, o firewall para de perguntar sobre o hangar-server a cada atualização
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\open-firewall.ps1 -NoPrompt
prova: ~/.hangar/firewall-ports
destrutivo: false
tipo: maquina
---

No Windows, cada atualização abria o aviso "Do you want to allow public and private networks to
access this app?" para o hangar-server, mesmo depois de ele ter sido permitido na versão anterior.
As portas 8765 e 8766 agora ficam liberadas no firewall, nas redes privadas e públicas, por uma
regra que vale para todas as versões. Onde o Hangar roda como administrador a regra entra nesta
atualização. Nos demais, a atualização não pede a senha de administrador: a regra entra quando o
instalador rodar, ou com o comando gravado em `%USERPROFILE%\.hangar\firewall-ports`, num
PowerShell aberto como administrador. Na instalação por pacote ele é
`powershell -ExecutionPolicy Bypass -File "%USERPROFILE%\.hangar\current\scripts\open-firewall.ps1"`;
na instalação pelo código, o mesmo script dentro da pasta do clone.

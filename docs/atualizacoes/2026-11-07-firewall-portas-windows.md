---
id: 2026-11-07-firewall-portas-windows
titulo: No Windows, o firewall para de perguntar sobre o hangar-server a cada atualização
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\open-firewall.ps1
prova: ~/.hangar/firewall-ports
destrutivo: false
tipo: maquina
---

No Windows, cada atualização abria o aviso "Do you want to allow public and private networks to
access this app?" para o hangar-server, mesmo depois de ele ter sido permitido na versão anterior.
As portas 8765 e 8766 agora ficam liberadas no firewall uma vez só, nas redes privadas e públicas;
o Windows pede a senha de administrador uma única vez para isso.

---
id: 2026-10-10-chaves-do-push-no-celular
titulo: As notificações no celular voltam a funcionar
comando_posix: scripts/hangar-server-command vapid-setup --root . && mkdir -p ~/.hangar/passos && touch ~/.hangar/passos/2026-10-10-chaves-do-push-no-celular
comando_windows: (if exist bin\hangar-server.exe (bin\hangar-server.exe vapid-setup --root .) else ("%USERPROFILE%\.hangar\bin\hangar-server.exe" vapid-setup --root .)) && (if not exist "%USERPROFILE%\.hangar\passos\" mkdir "%USERPROFILE%\.hangar\passos") && type nul >> "%USERPROFILE%\.hangar\passos\2026-10-10-chaves-do-push-no-celular"
prova: ~/.hangar/passos/2026-10-10-chaves-do-push-no-celular
destrutivo: false
tipo: maquina
---

O servidor não tinha as chaves que assinam as notificações, e o aviso de "terminou" nunca chegava
ao celular. Agora elas são geradas na primeira vez. Depois de atualizar, ative de novo em
Configurações → Notificações → "Ativar notificações em todas as máquinas", no Hangar do celular.

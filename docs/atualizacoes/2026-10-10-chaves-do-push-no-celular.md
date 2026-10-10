---
id: 2026-10-10-chaves-do-push-no-celular
titulo: As notificações no celular voltam a funcionar
comando: cd backend && uv run python -m app.vapid_setup
prova: backend/.env
destrutivo: false
---

O servidor não tinha as chaves que assinam as notificações, e o aviso de "terminou" nunca chegava
ao celular. Agora elas são geradas na primeira vez. Depois de atualizar, ative de novo em
Configurações → Notificações → "Ativar notificações em todas as máquinas", no Hangar do celular.

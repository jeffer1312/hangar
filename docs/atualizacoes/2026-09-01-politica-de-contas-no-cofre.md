---
id: 2026-09-01-politica-de-contas-no-cofre
titulo: A política de contas da orquestração mudou de ~/.claude para o cofre ~/.hangar
prova: backend/app/orq_politica.py
destrutivo: false
---

O arquivo `orquestracao-contas.md` (quais contas e modelos o time da orquestração pode usar)
passou de `~/.claude/` para `~/.hangar/`, junto do diário e dos anexos — é da máquina, não de
uma conta. O backend move o arquivo sozinho na subida; se já houver um nos dois lugares, ele lê o
do cofre e avisa no log sem apagar nada.

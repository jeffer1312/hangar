---
id: 2026-09-29-rede-local
titulo: As outras máquinas passam a ser alcançadas pela rede local quando ela é mais rápida que o Tailscale
comando_posix: ./install.sh --update
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: backend/.env
destrutivo: false
tipo: maquina
---

Quando você está na mesma rede de uma máquina (a do trabalho no trabalho, a de casa em casa), o
app conversa com ela direto pela rede local, sem a volta pelo Tailscale. Fora dela, nada muda.
Esta máquina passa a aceitar conexões da rede local, sempre com o token.

---
id: 2026-10-09-python3-antes-da-loja
titulo: No Windows, os hooks em Python do Claude voltam a rodar
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/python3
destrutivo: false
---

No Windows, o atalho do Python da Microsoft Store vinha antes do `python3` do Hangar no PATH, e
os hooks do Claude que chamam `python3` falhavam sem aviso. O `~/.local/bin` agora fica na frente;
vale nos terminais e sessões abertos depois da atualização.

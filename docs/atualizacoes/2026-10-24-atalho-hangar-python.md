---
id: 2026-10-24-atalho-hangar-python
titulo: Os scripts do Hangar passam a usar o Python do próprio Hangar pelo atalho hangar-python
comando_posix: ./scripts/install-hangar-send.sh
prova: ~/.local/bin/hangar-python
destrutivo: false
---

Os comandos do Hangar no terminal (`hangar-send`, `hangar-conta` e afins) passam a rodar com o
Python do próprio Hangar, pelo atalho `hangar-python`. Sem o atalho eles continuam usando o
`python3` da máquina, como antes. No Windows o atalho chega com a migração para o pacote.

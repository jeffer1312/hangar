# Rótulos de triagem

As skills falam em cinco papéis canônicos de triagem. Aqui não há sistema de labels — o tracker é
markdown versionado (ver `issue-tracker.md`), então o rótulo é uma linha `Status:` no corpo da
Task, com a string da coluna do meio.

| Papel na skill | String usada aqui | Significado |
| --- | --- | --- |
| `needs-triage` | `needs-triage` | Ainda não foi avaliado |
| `needs-info` | `needs-info` | Esperando informação do usuário |
| `ready-for-agent` | `ready-for-agent` | Especificado por inteiro, um agente pode pegar sozinho |
| `ready-for-human` | `ready-for-human` | Precisa de mão humana |
| `wontfix` | `wontfix` | Não vai ser feito |

Uma Task de plano nasce `ready-for-agent` — o formato do plano já exige critério de verificação por
Step, que é o que torna a Task pegável sem conversa. Task que só um humano fecha (conferir pixel na
tela, aprovar um mock) carrega **"verificação manual"** no título do Step, que é a marca que o
`planprog.py` já lê.

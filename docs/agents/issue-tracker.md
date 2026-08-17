# Issue tracker: markdown local, na convenção da casa

Este repo **não** usa GitHub Issues para o trabalho dirigido por agente, mesmo tendo remote no
GitHub e `gh` autenticado. Specs e tickets vivem como markdown versionado, em dois lugares que já
existiam antes destas skills:

| O quê | Onde | Quem mais lê |
|---|---|---|
| Spec / research | `docs/superpowers/specs/<AAAA-MM-DD>-<slug>.md` | o plano cita o arquivo |
| Plano com os tickets | `docs/superpowers/plans/<AAAA-MM-DD>-<slug>.md` | `backend/app/planprog.py`, `skills/orchestrating-idea-to-push` |
| Plano concluído | `docs/superpowers/plans/feitos/` | arquivo morto |

**Por que não `.scratch/` nem GitHub Issues:** o próprio app lê esse diretório. `planprog.py`
varre `docs/superpowers/plans/` e alimenta a barra de progresso do plano (chip 📋 no celular,
barra segmentada no card). E `skills/orchestrating-idea-to-push` recorta a Task da vez desse mesmo
arquivo para entregar ao executor. Ticket publicado em outro lugar sai do campo de visão das duas
coisas.

## O formato — ele não é livre

`planprog.py` casa duas expressões regulares, e o plano só aparece na barra se respeitar as duas
**exatamente**:

```
### Task 3: <título da Task>
- [ ] **Step 7: <título do Step>**
```

- Task: `### Task <n>: …` — três `#`, a palavra `Task`, dois-pontos.
- Step: `- [ ] **Step <n>: …**` — a caixa, e o `Step …` inteiro em negrito.
- Marcar `- [ ]` → `- [x]` ao terminar cada Step é o que move a barra. Sem isso o plano parece
  não ter começado.
- Step que precisa de conferência humana leva **"verificação manual"** no título (`_MANUAL_RE`).
- Bloco de código dentro do plano é ignorado na contagem (as cercas são removidas antes do regex),
  então exemplo de Step dentro de ``` não é contado por engano.

## Quando uma skill disser "publique no issue tracker"

- **`/to-spec`** → escreve `docs/superpowers/specs/<AAAA-MM-DD>-<slug>.md`.
- **`/to-tickets`** → **não** escreve um arquivo por ticket. Cada ticket vira uma
  `### Task <n>: …` dentro de **um** arquivo de plano em `docs/superpowers/plans/`, com os Steps
  dela como `- [ ] **Step <n>: …**`. As arestas de bloqueio viram a **ordem** das Tasks mais uma
  seção no topo do plano dizendo o que corre em paralelo e o que é serial (o precedente é
  `## Lotes — o que corre em paralelo`, em `feitos/2026-08-15-arvore-de-arquivos-e-diff.md`).
- O rótulo de triagem, quando existir, é uma linha `Status:` no corpo da Task — não há sistema de
  labels aqui.

## Quando uma skill disser "busque o ticket relevante"

Leia o arquivo do plano e recorte a Task pedida. **Nunca entregue o plano inteiro a um executor** —
medido em 14/08/2026: plano inteiro ~30k tokens, Task recortada ~2,9k.

## Operações de wayfinding

Não usadas neste repo. `/wayfinder` foi avaliado e descartado para este trabalho: a execução
multi-sessão já tem dono, a skill `orchestrating-idea-to-push`.

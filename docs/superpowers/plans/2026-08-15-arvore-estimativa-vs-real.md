# Estimativa × real — árvore de arquivos e diff

Registrado **antes** de a execução começar, em 15/08/2026, para não virar memória seletiva depois.
Plano: [`2026-08-15-arvore-de-arquivos-e-diff.md`](2026-08-15-arvore-de-arquivos-e-diff.md).

## Como a estimativa foi feita

Sem histórico de execução deste repositório com estes modelos — é julgamento, não medição. A conta:

- 85 passos no plano, contados um a um.
- Um ciclo teste-implementa-roda-commita: **20 a 45 min** num modelo rápido.
- Revisão independente: **15 a 25 min por commit**.
- Nos lotes paralelos conta **só a Task mais lenta**, porque as outras correm junto.
- As Tasks de tela levam o dobro: cada uma tem duas comparações cegas e pode pedir segunda rodada.

Não entra na conta: conflito de merge (vira Task nova serial), troca de sessão por cota ou por
contexto, e o tempo em que o usuário estiver dormindo.

## O que foi estimado

| Bloco | Tasks | Escritores juntos | Estimado |
|---|---|---|---|
| Preparar worktrees (`.venv` / `node_modules` por worktree) | — | — | 30–60 min |
| Lote A + revisões | 1, 2, 3 | 3 | 1h30 – 2h30 |
| Costura 1 | 4 | 1 | 40 min – 1h10 |
| Costura 2 | 5 | 1 | 30 – 55 min |
| Lote B + revisões | 6, 7, 8, 9 | 4 | 1h – 1h45 |
| Montagem (as 3 telas) | 10, 11, 12 | 1 | **3h – 6h** |
| Revisão final da branch | — | — | 30 – 60 min |
| **Total** | | | **8h – 14h de relógio** |

**Aposta declarada:** a montagem é metade do total e é a parte mais imprevisível. Se o total real
estourar 14h, a causa mais provável é ela — segunda rodada de comparação visual em mais de uma Task.

## O real — preencher DURANTE, não no fim

O árbitro anota ao fechar cada bloco. Hora de início e fim em relógio, não em duração calculada de
cabeça.

| Bloco | Início | Fim | Real | Estimado | Diferença |
|---|---|---|---|---|---|
| Worktrees | | | | 30–60 min | |
| Lote A | | | | 1h30 – 2h30 | |
| Costura 1 | | | | 40 min – 1h10 | |
| Costura 2 | | | | 30 – 55 min | |
| Lote B | | | | 1h – 1h45 | |
| Montagem | | | | 3h – 6h | |
| Revisão final | | | | 30 – 60 min | |
| **Total** | | | | **8h – 14h** | |

### Eventos que a estimativa não previa

Uma linha por evento, com o custo em tempo. É isto que faz a próxima estimativa ser melhor que
esta.

| Quando | O que aconteceu | Custo |
|---|---|---|
| | | |

### Contagem de rodadas visuais

O teto é 2 por Task. Anotar quantas cada uma precisou de verdade.

| Task | Rodadas de fidelidade | Rodadas de integração |
|---|---|---|
| 10 — aba no painel | | |
| 11 — arquivo cobrindo a conversa | | |
| 12 — aba no celular | | |

## Fechamento

Preencher quando a branch for aprovada: o total real, o erro da estimativa em porcentagem, e **uma
frase** sobre o que eu deveria ter previsto e não previ.

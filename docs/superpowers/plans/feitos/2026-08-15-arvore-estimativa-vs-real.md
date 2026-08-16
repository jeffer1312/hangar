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
| Worktrees | 14:11 | 14:20 | 9 min | 30–60 min | −21 a −51 min |
| Lote A | 14:20 | 15:36 | 1h16 | 1h30 – 2h30 | −14 min a −1h14 |
| Costura 1 | 15:40 | 19:38 | **3h58** | 40 min – 1h10 | **+2h48 a +3h18** |
| Costura 2 | 19:40 | 19:58 | **18 min** | 30 – 55 min | −12 a −37 min |
| Lote B | 20:05 | 21:02 | **57 min** | 1h – 1h45 | −3 min a −48 min |
| Montagem | | | | 3h – 6h | |
| Revisão final | | | | 30 – 60 min | |
| **Total** | | | | **8h – 14h** | |

### Eventos que a estimativa não previa

Uma linha por evento, com o custo em tempo. É isto que faz a próxima estimativa ser melhor que
esta.

| Quando | O que aconteceu | Custo |
|---|---|---|
| 15/08 15:2x | **O merge da Task 1 conflitou** em `backend/tests/test_git_ops.py`. O lote A nunca foi disjunto: `git_ops.py`/`test_git_ops.py` estavam na Task 3 por desenho e na Task 1 porque o Step 8 do plano mandava mexer em `changed_files`. O `self-review` do plano (item 5) declarava o lote disjunto. Devolvido ao executor como Task nova serial (merge da `main` na branch dele), revisado e mergeado. | ~35 min |
| 15/08 15:40–19:38 | **A Costura 1 levou 3h58 contra 1h estimada, em 10 rodadas.** Nove REPROVA na mesma família (impedir leitura de dentro da `.git`), cada rodada fechando o caso que o parecer anterior nomeou e a seguinte achando outro, com a solução crescendo até virar estado de sessão em memória no `api.py`. O usuário parou a espiral com a pergunta certa — *"não é um gerenciador de arquivos? Por que o git virou o centro?"* — e a simplificação (−283/+37, guard de 3 linhas por componente do caminho resolvido) foi **aprovada de primeira**. A estimativa não previa isto porque ela conta rodadas de revisão, não **espiral de escopo dentro de uma Task**: o que faltou não foi tempo de execução, foi um juízo de "esse caso vale o desenho?" no portão. | **+3h** |
| 15/08, o dia todo | **Dois arquivos de teste caem sob carga** (`test_termsock.py`, `test_shell_scripts.py`): subprocess/PTY sensível a tempo, com a máquina acima de 4 de load por causa do próprio paralelo. Custou 3 rodadas de suíte de um revisor sem nunca ver verde. Resolvido com portão de dois passos (`--ignore` + os dois isolados no relato). | ~25 min somados |
| 15/08 14:34 | Os três executores nasceram como **sessão Claude com motor `deepseek-direto`** (o que a tabela do plano diz literalmente) em vez de **sessão Pi com `opencode-go/deepseek-v4-flash`**. A linha de status é idêntica nas duas formas, então a prova de modelo não pegou — só o `pane_start_command` distingue. Worktrees ainda limpas: nenhum trabalho perdido. | ~14 min |

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

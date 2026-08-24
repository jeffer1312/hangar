# `opencode-go/ox-alpha-free` — papel: EXECUTOR

Primeira ficha: 22/08/2026, trabalho `mx` (app mobile Expo, 10 Tasks). Trabalhou em T5 (prova), T8
(1ª sessão) e T10 (1ª sessão), revezando com o muse-spark por ordem do usuário. Uma execução — tudo
marcado *(visto uma vez)* até a próxima confirmar.

## Números

- **Janela 1M** (catálogo do Pi: `openai-completions`, imagem). O bloco escrito à mão no
  `models.json` dizia 128k — **era configuração, não modelo**; removido, o catálogo embutido serve.
- **Custo: US$0,00** em todas as sessões (tier free).
- Thinking `max`. Conta `opencode-go` — autentica pela chave do **`auth.json`**, não do
  `models.json` (as duas eram diferentes; a do auth respondia 429 e a sessão morria com o curl passando).

## Enxerga imagem: SIM

Abre PNG com `Read` e julga. Na T5 entregou **a melhor prova visual da execução**: 6 estados abertos e
descritos de forma verificável, comparação cega rodada por subagente fresco, veredito confirmado pelo
revisor. Barra de Task de tela pode ser print; `see` não é necessário.

## Como ele falha — TRANSPORTE, não julgamento

- **9 quedas de stream em ~2h40 de trabalho pesado** *(visto em 3 sessões, 22/08/2026)*:
  `Stream ended without finish_reason`, `Provider finish_reason: network_error`, `503 (no body)` ×3 +
  `Retry failed after 3 attempts`. Ritmo: uma a cada ~30–40 min em Task longa. **3 fatais** (não
  reanimaram com cutucão): t5d (ctx congelado, turno morto) e t10 **no primeiro turno** (59 tokens de
  entrada, nada produzido).
- **Rendimento despenca depois da 2ª hora**: entre a 7ª e a 8ª queda da T8, 9k de contexto em 35 min
  — quase todo o relógio indo pra reconexão. Foi o gatilho de troca, não a queda em si.
- **A saída morre antes do reporte**: turno cai logo depois do commit; o reporte fica no pane.
- Não é problema do modelo sozinho: o muse-spark cai no mesmo provedor, menos (5× contra 9×).

## O que o kick-off precisa dizer por causa dele

- **Vigia armada é condição de uso, não rede** — sem cutucão automático nenhuma Task longa fecha.
- "Se a tua saída morrer, escreve o reporte em `report-task-N.md`; não gaste turno reenviando."
- **Orçamento de imagem**: o provedor recusa requisição com >50 imagens (`request contains 51
  images…`), sem volta dentro da sessão. Abrir só os PNGs que vai julgar; comparação em massa vai
  pra subagente fresco.
- Prova de modelo é o `pane_start_command`, não a statusline que a sessão relata (um executor
  relatou `ox-alpha-free` sendo `muse-spark`).

## Onde ele é bom — e onde não colocá-lo

- **Task curta e visual** (prova, recaptura, comparação): julgamento visual próprio, custo zero.
- **Não** em Task longa de código (T8 chat: 2h36 sem commit, 8 quedas; substituta no spark commitou em
  20 min com o trabalho dele no disco) e **não** como único executor de um lote — o revezamento
  acabou na T10 porque ele caiu antes da primeira linha.
- Estimativa de plano: contar **≥2 sessões por Task** quando ele é o executor.

## 2ª execução (paridade, 22–24/08/2026) — confirmado, e o papel mudou

- **Transporte confirmou-se fatal em Task longa:** 16 erros de provedor em 2 sessões na única Task
  que pegou (~3h30 perdidas), com a 2ª sessão caindo MAIS que a 1ª (6 erros em 21 min contra 10 em
  ~65). **Terceira sessão no mesmo modelo não se abre — a segunda já é a medição.**
- Rendimento entre quedas desta execução: **3k de contexto em 18 min** (1ª execução: 9k em 35 min).
- O que produziu entre quedas estava certo (commit completo, árvore limpa) — julgamento não é o
  problema.
- **Saiu da rotação de executor** (decisão do usuário, 23/08/2026, ao nomear a alternância entre os
  dois titulares). Papel atual: **fallback** da conta — e com ele de executor, a Task se estima em
  2–3× o relógio do modelo estável, ≥2 sessões, vigia armada, reporte em arquivo.

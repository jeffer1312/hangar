# Task 7 — rolagem suave da conversa

Executor: native-parity-task7-exec (claude-opus-5-5, esforço medium, `CLAUDE_CONFIG_DIR=/home/jefferson/.claude-claude-200-3`, sem terminal).
Base: `baseline-task7/` (Task 6 aprovada, com o conserto r2b). Provas em
`~/.hangar/orq/2026-09-23-native-parity/visual/task7-r1/`.

## Causa

- A lista usava `FollowMode::Tail` do gpui: a cada layout ela reposiciona no fim. Cada linha nova
  do texto, cada linha nova da conversa e cada remedida viravam um salto no mesmo quadro.
- A roda do mouse chega como `ScrollDelta::Lines` (Wayland: 3 linhas por entalhe) e a lista
  converte em 60 px aplicados num quadro só.
- "Ir para o fim" reposicionava no fim de uma vez (~500 px entre dois quadros, `s19-latest-jump.png`).
- Armadilha do gpui achada no caminho: com a lista colada no fim, a posição lógica fica além do
  último item e `ListState::scroll_by` parte da altura total, não do topo visível. Qualquer
  `scroll_by` para cima a partir dali cai além do fim e o layout volta a colar.

## O que mudou

`desktop-native/src/app/follow.rs` (novo) e os pontos de chamada em `app.rs`:

- **Mola até o fim.** `FollowMode` fica no padrão (`Normal`); o acompanhamento é nosso. Uma mola
  de velocidade (mesma forma e constantes do `StickSpring` do Zeron, reescrita aqui) desliza até
  o fim, um passo por quadro (`window.on_next_frame`, agendado no render só enquanto há
  distância a vencer ou conteúdo novo).
- **Conteúdo novo não salta.** Antes de cada mudança de altura (`sync_rows`, fim do parse do
  Markdown, mídia carregada) a lista colada é ancorada num pixel logo acima do fim
  (`unglue`); o layout mantém esse pixel e a mola desliza o crescimento. A mola pousa num pixel,
  sem colar no fim: colar no pouso deixava a linha nova que entrava antes do layout saltar
  (+159 px medidos no rascunho).
- **Leitura preservada.** Só gesto solta o fim: roda para cima solta na hora; trackpad/toque
  passam pela lista e o handler dela (adiado com `cx.defer`, a lista está emprestada durante
  ele) solta quando a distância cresce. Voltar por gesto a menos de 70 px do fim, descendo,
  cola de novo. Crescimento de conteúdo nunca solta.
- **"Ir para o fim" e envio** voltam deslizando; distância acima de 2,5 alturas da janela
  teleporta até 2,5 alturas e desliza o resto (mesma regra do Zeron).
- **Roda do mouse animada.** Uma camada `canvas` sobre a lista pega `ScrollDelta::Lines` na fase
  de captura (antes da lista) e anda a mesma distância de antes (20 px por linha) em ~150 ms
  (constante de tempo 45 ms). Pixels de trackpad seguem direto para a lista. A camada usa
  `hitbox.should_handle_scroll`, então elemento que ocluda a lista continua recebendo a roda.
- **Movimento reduzido** (`cx.reduce_motion()`): sem mola nem roda animada; fim colado como antes
  (corrigido na rodada 2).

O que não mudou: uma linha de lista por mensagem, janela de histórico, prévia em streaming
(Task 1), ferramentas recolhíveis (Task 2), layout da Task 6. A lista não foi dividida em uma
linha por bloco de Markdown (ver "Decidido").

## Provas

Fixture determinística: modo novo `scroll` em `tools/parity_stream_fixture.py` (24 mensagens em
português no histórico; resposta longa com título, lista, bloco de código e acentos, crescendo 2
palavras a cada 120 ms). Mesmo roteiro para todos (`roteiro/scenario.sh`): texto chegando com a
lista no fim; aos 5 s, 4 entalhes de roda para cima; "Ir para o fim"; parada, 6 entalhes para
cima e 6 para baixo. Gravação `wf-recorder` 60 Hz recortada na janela; `roteiro/motion.py` mede o
deslocamento vertical da coluna da conversa entre quadros consecutivos, `roteiro/phases.py`
resume por fase (`medidas/fases.txt`).

| Fase | antes (`s19-before.mp4`, task6-r2b) | depois, depuração (`s20-after.mp4`) | depois, otimizado (`s20-after-release.mp4`) |
|---|---|---|---|
| texto chegando, no fim | 9 saltos, maior 158 px | 35 passos, maior 69 px | 77 passos, maior 14 px |
| roda para cima com texto chegando | 4 saltos de 60 px | 21 passos, maior 39 px | 38 passos, maior 19 px |
| "Ir para o fim" | salto de ~500 px em 1 quadro (fora da faixa de 300 px da medida; folha `s19-latest-jump.png`) | 24 passos, maior 74 px | 42 passos, maior 34 px |
| roda parada, 6 entalhes | 6 saltos de 60 px | 25 passos, maior 39 px | 37 passos, maior 35 px |

Rolada para cima com texto chegando, a conversa fica parada nos três binários (sem passo positivo
entre a roda e o clique em "Ir para o fim"). O de depuração tem passos maiores porque cada quadro
dele leva 40–70 ms (medido no rastro do rascunho); o otimizado desenha a 60 Hz.

O que faria a prova falhar: no "depois", um quadro com passo do tamanho de uma linha ou mais
(≥ 26 px) na fase "texto chegando" do otimizado, ou qualquer passo positivo entre a roda para cima
e o clique.

**Sessão real** (`cx-t7-scroll`, conta 200-3, Opus 5.5 esforço low, sem terminal, criada por mim):
`real-before.mp4` (binário task6-r2b, só assistindo; o texto foi pedido por `hangar-send`, nada
digitado no nativo). Texto chegando: +26 px por linha, +234 px e +138 px num quadro; roda: 60 px
e 180 px num quadro. **O "depois" em sessão real não foi gravado**: o cursor passou a se mover
sobre o DP-1 (uso do usuário), dois cliques abortaram na conferência de posição antes de clicar, a
janela de prova foi fechada de fora às 08:15:23 e parei por ordem da árbitra.

Binários congelados: `bin/hangar-native-task7-r1` (depuração) e `bin/hangar-native-task7-r1-release`
(otimizado); as provas do "depois" são desses arquivos (hashes em `hashes-task-7-r1.txt`).

## Verificação

- `cd desktop-native && cargo build --locked` → exit 0, só os 2 avisos antigos (`problema`,
  `steered`/`native`).
- `cargo build --locked --release` → exit 0.
- `cargo test --locked --no-run` → exit 0; testes novos `spring_glides_monotonically_and_lands_on_the_target`
  e `spring_follows_steady_growth_without_falling_behind` compilados, nenhum executado.
- Revisor automático `ecc:code-reviewer`, uma vez, com a proibição de testes e de ler dados do
  app do usuário no prompt: nenhum defeito (conferiu a conta do `unglue`, o empréstimo da lista no
  handler, a camada da roda sob popover que oclui e o fim do agendamento de quadros).
- Janela de prova no DP-1/ws9 (vazio na abertura), cursor, PID, workspace e geometria conferidos
  antes de cada clique e roda; HDMI-A-1 (ws5) e eDP-1 (ws2) intocados.

## Decidido

- Uma linha por mensagem mantida (Zeron usa uma por bloco de Markdown): a mola resolve o salto
  medido, e trocar a granularidade mexeria no modelo de linhas das Tasks 1–2. Cache por impressão
  digital já existe (`rich` por id + assinatura).
- Mesma distância por entalhe de antes (60 px), só animada.
- Recomendo relançar a janela do usuário com o binário otimizado: no de depuração a mola funciona,
  mas cada quadro custa 40–70 ms.

## Rodada 2

Parecer `pareceres/task7-r1.md` (REPROVA, 2 bloqueios). Provas no notebook (eDP-1, ws27 próprio,
escala 1,25; o eDP-1 voltou ao ws1 em que estava). Fixture nova `scroll-long`: pensamento e
ferramenta ao vivo, depois resposta com três vezes o texto (mais de duas alturas de janela). Vídeos
em `visual/task7-r2/` (px de tela = px de vídeo ÷ 1,25; uma linha = 26 px de tela).

- **BLOCKER 1** (resposta alta terminando pula para o topo dela). Causa reproduzida no binário r1
  (`r1-long-pinned.mp4`): no fim do texto, −160 px, −160 px e depois +145, +66, +59 px (topo da
  resposta e mola voltando). Correção da receita: `splice_rows` (follow.rs) regrava a âncora no
  início do trecho trocado com a distância em pixels; `sync_rows` passa por ele (único `splice` do
  app). Depois (`r2-long-pinned.mp4`): no fim só −30 px (a linha "Trabalhando" saindo, NOTED 1,
  já existia) e nenhum movimento depois. Lendo dentro da resposta (`r2-long-reading.mp4`, 2
  entalhes para cima aos 20 s): depois da roda, nenhum passo até o fim da gravação, com o texto
  terminando no meio.
- **BLOCKER 2** (movimento reduzido ainda animava). A receita (colar no render) desfazia o gesto
  de subir: com ela, no binário anterior, o entalhe para cima não aparecia (arquivado em
  `visual/task7-r2/anterior/`). Divergência levada à árbitra, que decidiu: colar em
  `follow_content_changed` (movimento reduzido e colada → `scroll_to_end` antes de a altura mudar)
  e `schedule_scroll` só não agenda quadro. Prova (`r2-reduced.mp4`, cópia descartável com
  `set_reduce_motion(true)`, fora da entrega): cada linha nova é um passo único de 32 px de vídeo,
  sem deslize; entalhe para cima −75 px aparece e nada puxa de volta; entalhe para baixo volta ao
  fim e a lista segue colada.
- **Passo da mola limitado** (decisão da árbitra): no máximo 20 px de tela por quadro de 60 Hz,
  proporcional a quadros atrasados; teste `late_frames_move_proportionally_but_never_past_the_target`.
  Medido: maior passo por quadro 20,0 px (colada) e 10,7 px (lendo), contando quadros em que a
  coluna repintou. Passo bruto visível: 32 px (colada, depois de um quadro perdido) e 66 px
  (lendo, em 4,8 s, depois de 134 ms em que o app não repintou a coluna quando a linha nova da
  resposta entrou com o título em Markdown). Esses saltos vêm do custo de desenhar, não da mola;
  o r1 tinha o mesmo (71 px brutos). Uma linha por bloco de Markdown, como no Zeron, é o caminho
  para eles e fica fora desta Task.
- **NOTED 2** (trackpad soltando o fim com texto chegando): o gesto passa a ser medido pelo topo
  visível, que texto novo não mexe (`last_top`, atualizado a cada render). Não provado em tela:
  ydotool não gera rolagem em pixels.
- **NOTED 3**: roda para cima numa conversa que cabe na janela não solta mais o fim.

## Não conferido

- Trackpad (ydotool não gera rolagem em pixels): o caminho é o da lista, sem mudança além da
  decisão de soltar/colar.
- "Depois" em sessão real (acima).

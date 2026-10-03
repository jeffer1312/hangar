# Correções da revisão 2C

Base `d21a445b`, branch `hangar-server-parte2c`. Executar com regressão primeiro e commit descritivo por item, sem push. Somente testes focados, processos HTTP sintéticos e tmux em socket privado; nenhum serviço real. A promoção exige menor CPU total e menos processos por rodada/chat que um capture-pane Python. Baseline congelada em /tmp antes da simplificação, custo de startup separado, mesma carga/conteúdo/checagem de resultado, CPU Python+Rust+tmux/filhos, comandos e HTTP contados, repetição de lotes.

### Task 1: Retomar prévia/estado em divergência transitória de /clear
- [x] **Step 1: Reproduzir Python com dois SSE em tempos diferentes**
- [x] **Step 2: Pular rodada obsoleta sem encerrar produtores**
- [x] **Step 3: Testes focados e commit do item 1**

### Task 2: Excluir clientes controle de presença/identidade no painel e scripts
- [x] **Step 1: Inventariar leitores e regressão com dublê tmux**
- [x] **Step 2: Filtrar control-mode com decisão conservadora em erro**
- [x] **Step 3: Testes focados e commit do item 2**

### Task 3: Falhas persistentes sem repetição de processos
- [x] **Step 1: Regressão de fonte sem capacidade ou com falha persistente**
- [x] **Step 2: Pausa crescente de sessão, recuperação e versão mínima**
- [x] **Step 3: Testes focados e commit do item 3**

### Task 4: Ponte travada sem bloquear estado/executor/lock
- [x] **Step 1: Regressão de timeout/rajada e outra sessão independente**
- [x] **Step 2: Limitar espera, circuito temporário e liberar mutex antes de aguardar**
- [x] **Step 3: Testes focados e commit do item 4**

### Task 5: Remover observador Codex sem consumo
- [x] **Step 1: Provar que o monitor nativo não precisa iniciar observação**
- [x] **Step 2: Remover lease/heartbeat preservando eventos nativos e fechamento**
- [x] **Step 3: Testes focados e commit do item 5**

### Task 6: Reduzir custo por rodada e medir
- [x] **Step 1: Baseline congelada CPU/processos/comandos/HTTP por chat/rodada**
- [x] **Step 2: Simplificar captura e eliminar grade/cálculo/HTTP redundantes**
- [x] **Step 3: Paridade e testes focados; repetir baseline/vencedora**
- [x] **Step 4: Registrar medição em harnesses.md e commit do item 6 somente se vencer**

### Task 7: Registrar causa e transições da reserva
- [x] **Step 1: Reproduzir status/timeout/causa perdida e volta ao sucesso**
- [x] **Step 2: Diagnóstico seguro Rust/Python, entrada/saída e reset do aviso**
- [x] **Step 3: Testes focados e commit do item 7**

### Task 8: Heartbeat sem exceção engolida
- [x] **Step 1: Reproduzir falha do resolver no heartbeat**
- [x] **Step 2: Registrar tipo e continuar em reserva**
- [x] **Step 3: Testes focados e commit do item 8**

### Task 9: Falha inicial da lease não derruba produtores
- [x] **Step 1: Reproduzir falha do alvo no start**
- [x] **Step 2: Reserva fechada/segura e processamento Python**
- [x] **Step 3: Testes focados e commit do item 9**

### Task 10: Menores e revisão final
- [x] **Step 1: Conferir constantes Python/Rust e evicção sem reset alheio**
- [x] **Step 2: Registrar encontro de protocolo com sessão rust-parte2**
- [x] **Step 3: Revisão independente de mudanças e evidência de custo; corrigir problemas**
- [x] **Step 4: Estado real da árvore e aviso à sessão hangar**

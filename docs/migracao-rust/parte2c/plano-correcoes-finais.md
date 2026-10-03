# Correções finais da 2C

Base e0c44443, branch hangar-server-parte2c. Regressão antes, testes focados, commit por item, revisão independente final, sem push. Servidores/dados sintéticos, nunca serviço ou tmux do usuário; guardas antes de I/O. Preservar contrato3 e captura única por rodada.

### Task 1: Isolar falhas e transições por sessão
- [x] **Step 1: Reproduzir sessão ruim intercalada com boa e inundação do diário**
- [x] **Step 2: Contadores/pausas/reserva/avisos por sessão e warn Rust limitado**
- [x] **Step 3: Conferência focada e commits separados Python/Rust do item1**

### Task 2: Retentar início de lease sem reconectar SSE
- [x] **Step 1: Reproduzir falha inicial seguida de sucesso com mesmo produtor**
- [x] **Step 2: Separar reserva transitória de fechamento definitivo, preservando geração/cancelamento**
- [x] **Step 3: Conferência focada e commit do item2**

### Task 3: Acquire não apaga histórico de falha Rust
- [x] **Step 1: Reproduzir falha→pausa→acquire válido→falha e comparar duração**
- [x] **Step 2: Zerar apenas após captura válida**
- [x] **Step 3: Conferência focada e commit do item3**

### Task 4: Cancelamento externo na limpeza
- [x] **Step 1: Reproduzir cancelamento externo durante await do heartbeat**
- [x] **Step 2: Relançar quando a tarefa atual recebeu cancelamento**
- [x] **Step 3: Conferência focada e commit7b94f84a**

### Task 5: Referência Rust para 2B
- [x] **Step 1: Conferir chamadas de produção dos reducers/memória/fatos**
- [x] **Step 2: Marcar claramente referência sem inventar uso de produção**
- [x] **Step 3: Conferência focada e commit do item5**

### Task 6: Revisão e conclusão
- [x] **Step 1: Revisão independente do conjunto e correção de problemas concretos**
- [x] **Step 2: Conferir árvore/commits e registrar provas**
- [x] **Step 3: Avisar sessão hangar, sem push**

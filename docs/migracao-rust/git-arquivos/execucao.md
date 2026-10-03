# Git e arquivos em Rust — execução

Base: PR #24, commit `128262597d926416cb3a5e91f5ac3a2f99b798e1`.

## Entregas

- [x] Fase 0: branch e worktree isoladas, sem alterar o checkout compartilhado.
- [x] Núcleo compartilhado e contratos.
- [x] Git completo, incluindo consumidores internos.
- [x] Árvore, busca, leitura e escrita.
- [x] Citações e conteúdo servido.
- [x] Rotas, ponte privada e reserva Python.
- [x] Paridade, uso real, Windows e revisão independente.

## Decisões

- Protocolo 10 após integrar a parte 2B (7). As partes 3 e 2D reservam 8 e 9; ambos os lados mudam juntos.
- O núcleo usa operações tipadas e um executor síncrono limitado no servidor, fora do laço HTTP. O desktop conserva seu executor de tarefas.
- O repasse privado recebe caminhos já autorizados pelo chamador; não consulta novamente o registro de sessões, evitando ciclos.
- A reserva Python permanece durante a migração. Falha de transporte em alteração enviada não repete a operação.

## Verificação

Resultados, limites e medições em `verification.md`.

## Correções verificadas

- Cada achado da revisão independente teve reprodução e correção com regressão verde.
- Rascunho, erro e salvamento do modal de arquivos no PWA foram ligados ao store existente;
  a regressão falhou antes da mudança e passou depois, e o uso real confirmou o disco.
- Testes de concorrência falharam com as vagas únicas e a trava global do cache; passaram
  com a separação de vagas e trava por cwd.
- A auditoria posterior encontrou paridade incompleta de Range. A regressão falhou antes
  da correção e passou depois, incluindo erro 400 e união de intervalos sobrepostos.

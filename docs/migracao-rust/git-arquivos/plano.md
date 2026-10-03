# Migração de Git e arquivos — plano executado

## Fase 0

Worktree e branch `feat/rust-workspace-io` a partir do PR #24, sem trocar o checkout compartilhado.
O SHA de origem e o andamento ficam em `execucao.md`.

## Entregas

1. Contratos compartilhados, executor de comandos e núcleo `hangar-workspace`.
2. Git completo: consultas, grafo, diffs, ações, commit/amend, branches/tags e helpers de worktree.
3. Árvore, scanner, busca, leitura com digest e substituição atômica.
4. Citações, resolução autorizada, conteúdo servido com Range/cache e isolamento de HTML.
5. Rotas públicas, metadados no Python, ponte privada e reserva sem repetição de alterações.
6. Adaptadores do desktop, consumidores Python e dependência do núcleo no workflow do nativo.
7. Paridade de respostas/efeitos, regressões da revisão, uso real e verificação em Windows.

## Aceite

Os contratos e efeitos no disco são comparados em repositórios sintéticos. A fixture da interface
recusa todas as chamadas de arquivos/Git ao Python: a tela funcional, com contador zero, prova
que essas operações vieram do Rust. Testes de transporte cobrem resposta perdida, CORS,
campos extras, desligamento do executor e consultas durante operações lentas.

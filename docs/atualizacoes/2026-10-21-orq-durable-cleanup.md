---
id: 2026-10-21-orq-durable-cleanup
titulo: Orquestrações encerradas liberam builds e caches, preservando os relatórios
comando: uv run --project backend --no-sync python -c "from pathlib import Path; p = Path.home() / '.hangar' / 'orq-prune-request-v1'; p.parent.mkdir(parents=True, exist_ok=True); p.touch()"
prova: ~/.hangar/orq-prune-request-v1
destrutivo: true
tipo: maquina
---

A atualização agenda para a próxima inicialização do servidor novo a remoção de saídas de
compilação, caches de pacotes e sandboxes das orquestrações
antigas encerradas. Relatórios, registro, eventos, logs e capturas continuam disponíveis.
Execuções ativas, retomadas ou com registro ilegível são preservadas; o resultado da limpeza
lista o que foi removido e os impedimentos encontrados.

As provas novas usam uma pasta temporária fora do histórico durável, removida ao terminar a
rodada. O encerramento confere que não restou material regenerável.

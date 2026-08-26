---
id: 2026-08-26-orquestracao-e-arquivos-citados
titulo: Modal Orquestração (papéis e contas do time) e aba Arquivos com "citados" e ícones
prova: frontend/src/lib/fileIcons.generated.ts backend/app/orq_politica.py
destrutivo: false
---

Novo modal **Orquestração** (na barra da sessão e em Configurações → Servidor): você escolhe, por
papel do grupo (árbitro, executor, revisor…), onde roda (Claude, Codex, Pi, Kimi), a conta, o
modelo e o esforço — e ao salvar o árbitro recebe o recado com o que fazer. A aba **Contas
liberadas** grava a política da máquina (`~/.claude/orquestracao-contas.md`). A aba **Arquivos**
ganhou a visão **citados** (tudo que apareceu na conversa, com quem tocou), ícones por linguagem,
breadcrumb, pastas lembradas e skeleton ao carregar. Nada a fazer na máquina: o próprio
Atualizar já reinstala as dependências e reconstrói o app.

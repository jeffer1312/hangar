---
name: telemetria-triagem
description: |
  Lê a telemetria anônima do Hangar no receptor (Postgres na VPS, só leitura), ordena as falhas
  pelo número de máquinas afetadas, junta o contexto (campos, trilha, versão, sistema, estado da
  máquina) e acha no código onde cada falha nasce e se já foi corrigida. Use quando o dono pedir
  para ver onde mais dá erro, "triagem da telemetria", "o que está quebrando nas máquinas",
  ou para escolher o que corrigir a partir dos dados reais. Não serve para os relatórios de
  diagnóstico por e-mail (esses ficam no KV do worker hangar-relatorio).
---

# Triagem da telemetria

Os dados são anônimos e agregados por dia: nunca há texto de conversa, conta ou caminho. O que
existe está em `docs/telemetria.md` (eventos, dimensões, campos do diário) e no `wire.rs` do
crate `hangar-telemetry`.

## 1. Consultar (só leitura)

`SKILL_DIR/scripts/consultar <consulta> [dias]` (padrão 7 dias) roda no banco da VPS por SSH, com
a transação travada em só leitura. Consultas:

| Consulta | O que traz |
|---|---|
| `resumo` | máquinas, pacotes, versões, sistemas e canais do período |
| `falhas` | falhas do diário (`records`): evento, código, etapa, vezes, máquinas, sistemas, versões, primeira/última |
| `contadores` | eventos de falha contados (`problem_shown`, `session_open_failed`, `send_failed`, `client_fatal`, `update_failed_code`, …) por código |
| `logs` | linhas de aviso e erro do log do servidor (já limpas), por alvo |
| `detalhe <evento> [codigo]` | cada ocorrência da falha com campos, trilha antes dela e o ambiente do pacote |
| `sql "<select>"` | consulta livre, também só leitura |

Nunca escreva no banco (o script recusa; não contorne). Apagar ou corrigir dado é pedido do dono.

## 2. Validar antes de concluir

- **Versão:** compare a versão da falha com a atual da `main` (`git describe`/`VERSION`); falha
  só em versão antiga pode já estar corrigida. O `commit` do pacote diz de que código ela saiu.
- **Canal:** `latest` é release; outro canal é checkout de desenvolvimento e pesa menos.
- **Alcance:** uma máquina só, num dia só, é pista, não padrão. Ordene por máquinas, depois vezes.
- **Pacote parcial** (`partial`) é o dia ainda aberto: os números do dia sobem até o envio final.
- **Escondido x mostrado:** `problem_hidden` não chegou à tela (diagnóstico); `problem_shown` sim.
- Sem dado suficiente para afirmar, diga o que falta (e se a telemetria precisa medir mais).

## 3. Achar no código

Para cada falha relevante: `git grep` do código/evento (são textos fixos do binário) em
`crates/`, `desktop-native/src`, `mobile/src`, `packages/`; leia o produtor e o que leva até
ele; `git log <commit do pacote>..origin/main -- <arquivos>` mostra se mudou depois. Leia a
regra do `CLAUDE.md` e a entrada de `docs/decisoes/` da área antes de propor conserto.

## 4. Entregar

Tabela curta, da mais grave para a menos: o que a pessoa sofre (em português do dia a dia),
máquinas/vezes/sistemas/versões, onde nasce (`arquivo:linha`), se já foi corrigida, hipótese de
causa e como reproduzir (a trilha antes da falha). Diga a metodologia: período, tabelas e filtros.
Depois pergunte ao dono quais corrigir; cada conserto aprovado vira uma sessão própria
(`new_session` com `link`, branch nova da `main`), com a evidência da triagem no kick-off.

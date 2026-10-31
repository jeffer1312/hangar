# Fixtures da busca e do Arquivo

`home/` é um HOME de teste com transcripts reais desta máquina, cortados em poucas linhas. As
linhas foram copiadas byte a byte e passaram por um filtro de privacidade. As exceções estão
marcadas com ⚠ abaixo. Duas contas Claude (`.claude-a` é a do processo, `.claude-b` é a outra),
Codex padrão, Codex gerenciada `b`, Pi, omp e Kimi.

| Arquivo (sob `home/`) | Caso |
|---|---|
| `.claude-a/projects/-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1/0cc60e6c-….jsonl` | Conversa normal com acento (`árbitro`, `não`). É a viva na captura (`sessao-viva`) |
| `.claude-b/projects/…-9c0b634f-t1/0cc60e6c-….from-a.jsonl` | Cópia `.from-<conta>` que a exclusão da conta deixa (F4). ⚠ É a mesma cópia do arquivo acima |
| `.claude-b/projects/-home-jefferson-Projetos-hangar-mobile-deliveries-cad3e6fe-t32/db401d8f-….jsonl` e `.claude-b/projects/-home-jefferson-Projetos-hangar/db401d8f-….jsonl` | Mesma sessão, com os mesmos ids, em duas pastas de projeto (F1). ⚠ O mesmo corte foi copiado para as duas pastas. A cópia em `-home-jefferson-Projetos-hangar` tem nome fechado na captura |
| `.claude-b/projects/-home-jefferson-Projetos-hangar/a5a5a5a5-0000-4000-8000-000000000001.jsonl` | Conversa interna do Hangar (o Perguntar). ⚠ A 1ª linha foi montada a partir da linha 16 de `db401d8f`: o `message.content` foi trocado por `search.build_ask_prompt(...)`. O nome do arquivo também foi inventado |
| `.claude-a/projects/-home-jefferson/42a7c450-….jsonl` | Linha com mais de 40000 bytes (imagem colada mais o texto "ponytail"). Serve também à rota `transcript-image` (uuid `e73695a8-…`, idx 0) |
| `.claude-a/projects/-home-jefferson/f2afaa93-….jsonl` | Reserva pré-aquecida, sem nenhuma linha `type:user` |
| `.claude-b/projects/-tmp-cx-probe-work/f8f12de6-….jsonl` | Prompt barrado (`held:9decf68c`). ⚠ A linha 10 está repetida, como nas várias tentativas de envio. ⚠ A 4ª linha é a metade de uma linha real (não-JSON) |
| `.claude-a/projects/-home-jefferson-Projetos-hangar/75fea57d-….jsonl` | Reescrita: as linhas 9083 e 9084 do original (22:37:58) são descartadas pelo `RewriteFilter` (o `/compact`, id `1c3fcd4c-…`). ⚠ A linha 9085 (22:39:12) foi posta antes delas, no lugar do resumo do `/compact`, que tem texto privado |
| `.claude-a/projects/-home-jefferson-Projetos-hangar/75fea57d-…/subagents/agent-a23f817306f1d1b6a.jsonl` | Subagente (termo `fxtwitter`): fica fora da busca. ⚠ Vem de outra sessão |
| `.claude-a/.credentials.json`, `.claude-b/.credentials.json` | `{}`: o que o auto-scan de contas exige |
| `.codex/sessions/2026/08/30/rollout-…-01a05511-….jsonl` | Codex na conta padrão, cwd `/home/jefferson/Projetos/hangar` (mesma pasta do Claude). Sem as linhas 2–4 do original (instruções do usuário) |
| `.codex-b/sessions/2026/09/23/rollout-…-01a0d000-….jsonl` e `.codex-b/.hangar-codex-conta` | Codex na conta gerenciada `b`. ⚠ A pasta original da conta foi renomeada para `.codex-b`. Sem as linhas 2, 5 e 6 do original |
| `.pi/agent/sessions/--home-jefferson-Projetos-hangar-.worktrees-contas-modelos--/…_d337d6b8-….jsonl` | Pi |
| `.omp/agent/sessions/-tmp-claude-1000--…-omp-teste/…_01a06932-….jsonl` | omp |
| `.kimi-code/sessions/wd_hangar_5112ff7a84e0/session_ab697d05-…/agents/main/wire.jsonl` e `.kimi-code/session_index.jsonl` | Kimi. O wire fica sem as linhas de perfil, requisição e uso, que têm chaves. ⚠ No índice, `sessionDir` usa o marcador `{HOME}`, que quem copia troca pelo HOME de teste |

Não tem caso real nesta máquina: transferência Codex (não há registro em `conversation-transfers`)
e fila `queue-operation` limpa.

# Migração do backend para Rust — roteiro e estado

Atualizado em 2026-10-03. Ponto de partida para cada parte nova: ler este arquivo, a análise
inicial e a spec da parte 1. Tudo desta pasta vive na branch do PR #24 (`hangar-server-parte1`):
um pull nela traz a documentação em qualquer máquina.

## O que tem nesta pasta

| Arquivo | O que é |
|---|---|
| `analise-inicial.md` | Medições e motivos da migração, as 7 partes |
| `parte1/spec.md`, `parte1/plano.md` | Conversas do Claude/Codex no Rust, `hangar-cano`, tipos compartilhados |
| `parte1/registro-da-execucao.md` | Decisões tomadas na execução, detalhes adiados, o que ficou para conferir |
| `parte2-claude-codex/` | Análise e spec da parte 2 (2A–2D) e o plano da 2A executado |
| `parte2b/` | Análise, spec e plano da 2B (sem terminal no Rust) — aguardando aprovação |
| `parte2c/` | Análise, desenho, planos, revisões e provas da 2C (com terminal no Rust) |
| `parte2-descartada-lista-pi-kimi/` | Primeiro escopo da parte 2 (lista + Pi/Kimi/omp/orq), trocado pelo dono |
| `pedidos/` | Pedidos enviados às sessões Codex e os achados de revisão da 2C |

Os caminhos absolutos dentro de `pedidos/` e dos documentos das sessões apontam para as pastas de
trabalho da máquina de origem; nesta pasta os arquivos equivalentes são os da tabela acima.

## Como a migração funciona

- **Estrangulamento.** O `hangar-server` (Rust) fica na porta 8765. Ele atende sozinho o que já foi
  portado e repassa o resto ao Python, que escuta numa porta interna. O Python sobe e vigia o Rust
  e assume a porta sozinho se ele faltar ou cair. A cada parte, mais rotas passam para o Rust; no fim
  o Python sai e o instalador leva um binário só.
- **Contrato interno versionado à mão.** Mudou rota `/internal`, evento do `side-events` ou variável
  passada ao filho → subir `RUST_SERVER_PROTOCOL` (Python) e `INTERNAL_PROTOCOL` (Rust) juntos.
  O `versao` do snapshot do `hangar-cano` acompanha o `VERSAO` do `cano.py`.
- **Paridade provada por golden.** Formato que o cliente lê sai igual ao do Python, conferido por
  fixtures sintéticas geradas pelos parsers Python (`backend/tests/fixtures/contract/`).

## Partes

| # | Parte | Estado |
|---|---|---|
| 1 | Conversas do Claude e do Codex (`/history` e `/events`) + `hangar-cano` + tipos compartilhados (`crates/hangar-api`) | **Feita, no PR #24.** Rodando na máquina do dono pelo canal de testes. Falta teste no uso real e VM Windows |
| 2 | Sessões do Claude e do Codex, com e sem terminal — Pi, Kimi, omp, orq, lista, quadro e canvas ficam para depois (dono não usa agora) | Dividida em 2A, 2B, 2C e 2D. **2A** (fim da travada da prévia, no Python): feita, no PR. **2C** (com terminal: tmux `-C` no Rust, contrato versão 3): feita, no PR; testes verdes em Linux, Windows e macOS desde `16f45cad` (no Windows a 2C fica desligada de propósito e o Python atende). **2B** (sem terminal no Rust: cano, fila e controle; contrato versão 4): em execução na sessão `rust-parte2b`, branch `hangar-server-parte2b`, plano em `parte2b/`. **2D** (envio ao Claude com terminal): só desenho |
| 3 | Custos e uso (varredura de 38 s → 1–3 s estimado) | — |
| 4 | Estado e prévia (tmux em modo controle, cópia da tela por sessão) + terminal real (`portable-pty`) | — |
| 5 | Adaptadores dos provedores e envio de mensagens (mais mudam com as CLIs; tipos gerados do schema do app-server do Codex) | — |
| 6 | Resto da API: contas, convidados/8766, pareamento, MCP (`rmcp`), push (`web-push`), atualização | — |
| 7 | Remover o Python: binário único no instalador | — |

Cada parte: spec própria → plano → execução por subagente com revisor por Task → revisão final →
PR em rascunho → teste no uso real pelo canal de testes (abaixo) → merge.

## Testar uma branch no app (canal de testes)

Já na `main` (`c1751412`): `CP_UPDATE_BRANCH=<branch>` no `backend/.env` faz o "Atualizar" puxar a
branch em vez da `main` (com resgate, stash e rollback iguais). Fora da `main` a tela é compilada na
máquina. Esvaziar o campo volta para a `main` no próximo Atualizar manual. O backend lê o `.env` só
ao subir: depois de editar, reiniciar.

Na `hangar-server-parte1`, o `server.yml` compila a branch a cada push e publica em
`server-<branch>`; o download dos binários escolhe a release pela branch do checkout.

## Medição Python × Rust (parte 1, 02/10/2026)

Mesmos arquivos reais (lidos só na máquina, saída idêntica nos dois lados); mediana de 3 rodadas,
processo novo por rodada, tempo só da chamada; i5-13400F com outras sessões rodando (ruído 5–10%);
memória = pico do processo (o Python tem ~46 MB só de imports). "8 juntos" = 8 threads num processo,
como o backend faz. Rust = build glibc (o zigbuild 2.28 mediu igual).

| Arquivo | Leitura | Python | Rust | Ganho | Pico de memória Python → Rust |
|---|---|---|---|---|---|
| Claude 286 MB | histórico inteiro | 378 ms | 268 ms | 1,4× | 54–92 MB → 11 MB |
| Claude 286 MB | últimas 200 | 53 ms | 36 ms | 1,5× | → 9 MB |
| Claude 286 MB | 8 juntos | 3572 ms | 543 ms | 6,6× | → 49 MB |
| Claude 286 MB | chat ao vivo (arquivo todo) | 657 ms | 363 ms | 1,8× | → 11 MB |
| Codex 66 MB | histórico inteiro | 380 ms | 229 ms | 1,7× | 67–108 MB → 18 MB |
| Codex 66 MB | últimas 200 | 188 ms | 25 ms | 7,5× | → 8 MB |
| Codex 66 MB | 8 juntos | 2312 ms | 379 ms | 6,1× | → 90 MB |
| Codex 66 MB | chat ao vivo | 248 ms | 214 ms | 1,2× | → 14 MB |
| Claude 3,5 MB | histórico inteiro | 16 ms | 11 ms | 1,5× | 47–53 MB → 6 MB |
| Claude 3,5 MB | últimas 200 | 24 ms | 12,5 ms | 1,9× | → 6 MB |
| Claude 3,5 MB | 8 juntos | 154 ms | 41 ms | 3,8× | → 11 MB |
| Claude 3,5 MB | chat ao vivo | 18 ms | 12 ms | 1,5× | → 5 MB |

No histórico do Claude grande, os dois lados pulam a maior parte do arquivo por um atalho de regex
(`silent_attachment_timestamp`); o chat ao vivo lê tudo. O ganho grande é em leituras simultâneas,
onde o GIL serializa o Python.

## Lições da parte 1 (valem para as próximas)

- **Medir antes de afirmar.** O ganho real do Rust aqui é concorrência (6–9× em leituras
  simultâneas) e memória (10 MB contra 54–92 MB numa leitura grande); uma leitura sozinha ganha
  1,2–1,8×, porque o Python já usava atalhos.
- **Build Linux:** glibc 2.28 via `cargo zigbuild` (roda em Debian 10+, Ubuntu 20.04+, RHEL 8+).
  musl foi testado e descartado: o alocador dele custou até 5× mais memória em leituras simultâneas.
- **Todo binário novo tem reserva.** O `hangar-cano` é testado uma vez antes de usar e cai no
  `cano.py` se não rodar; o `hangar-server` cai no Python. Nada novo pode deixar a porta ou a sessão
  sem dono.
- **Log nunca leva texto de conversa:** pânico registra só arquivo:linha; erro de serde não vai cru.
- **Execução:** o código de port vindo pronto no plano (compilado antes numa cópia) fez as Tasks
  passarem de primeira; as revisões acharam o que importava em segurança, concorrência e paridade.

## Pendências da parte 1

- **Uso real** (roteiro em `docs/decisoes/plataforma.md`, entrada "hangar-server"): abrir chats do
  Claude e do Codex no celular, web e nativo; derrubar a rede e reconectar sem perder nem duplicar;
  reiniciar o backend com sessão sem terminal viva; `CP_RUST_SERVER=0`; medidas de "depois".
- **VM Windows:** cano abrindo sessão com `hangar-engine.CMD`; `scripts/windows-tasks.ps1`;
  `install.ps1` (sem `pwsh` aqui, nunca rodou); volta do Python para a 8765 depois de queda.
- **CI:** primeira execução do `server.yml` no GitHub (Zig, cargo-zigbuild, release por branch).
- **29 detalhes menores** anotados no registro da execução
  (`parte1/registro-da-execucao.md`),
  nenhum bloqueante. Os mais prováveis de aparecer: sem checagem de vida do Rust depois que sobe;
  binário presente mas incompatível só é detectado no `hangar-cano`.
- **Achados fora da migração:** na `main`, `cargo test` do `desktop-native` não compila
  (`src/app.rs:5790` usa `HashSet` sem import); 3 bugs do Python que derrubam o chat de uma sessão
  (surrogate solto com timestamp no `RewriteFilter`, linha JSON que não é objeto, U+2028 na fila).
- **Rust 1.99.0** saiu; a atualização (crates + nativo juntos) fica para um trabalho separado.

# Backend do Hangar em Rust — análise completa (02/10/2026)

Medido nesta máquina (16 núcleos, 31 GB) contra o backend vivo (PID 403147, só leitura e GETs)
e com benchmarks avulsos em Python 3.14. "Est." = estimativa, não medido. Rust não foi
compilado nem medido aqui: os ganhos em Rust são estimativas por crate equivalente.

## Conclusão

Vale migrar, mas **não pela velocidade de CPU no uso de hoje** (o backend usa ~1% de um
núcleo). Os ganhos reais, em ordem de peso:

1. **Teto de escala estrutural.** Cada chat aberto prende 2 threads e 2 inotify; com o limite de
   200 em `api.py:301`, ~100 chats simultâneos congelam a API inteira sem erro. Em Rust, um
   watcher por pasta e broadcast de frame pronto: custo fixo por cliente na casa de KB.
2. **Memória.** Backend 285–312 MB hoje → 30–60 MB (est.). Cada sessão headless tem um
   `cano.py` de 22 MB e 6 threads → ~2–3 MB (est.); com 100 sessões, 2,2 GB → 0,25 GB.
3. **Travadas por GIL.** Leitura de histórico em paralelo faz o laço de eventos atrasar até
   16 ms (medido, 8 em paralelo); a varredura fria de custos prende o GIL por 38 s.
4. **Distribuição.** Binário único: some Python 3.14 + uv + venv de 87 MB do instalador e do
   `atualizar.py`, e somem as regras de Windows que só existem por causa do Python.
5. **Contrato tipado.** O JSON da API está escrito à mão três vezes (Python, `packages/core`,
   `desktop-native/src/api/dto.rs` com 47 structs) e o nativo já duplica `git_ops.py`. Um crate
   compartilhado vira um contrato que o compilador confere.

Onde Rust **não** ajuda: o custo de chamar `tmux`/`git` (é do processo externo), e ~51 das 56
regras de `harnesses.md`, que descrevem manias das CLIs e valem em qualquer linguagem.

## Medições por área

| Área | Custo medido hoje | Ganho em Rust | Veredito |
|---|---|---|---|
| HTTP/API (`api.py`, 9k linhas, ~300 rotas) | 1 processo, 1 núcleo (`workers=1`); import 0,9 s e 112 MB antes de servir; sem checagem de tipo no CI | axum em todos os núcleos, ~20 ms de partida, rotas por domínio, porta de convidado com roteador próprio | portar redesenhando |
| Conversas/histórico (`transcript.py`, `pqueue.py`) | 300 MB completo 362 ms; Codex 69 MB 265 ms (80% em `_event_id`, `rollout.py:40`) | serde tipado pulando conteúdo: 5–10× (est.) | portar redesenhando |
| SSE (`sse.py`) | 10 tarefas + 2 watchers + 2 threads por conexão; cada conexão reparseia as mesmas linhas | um tail por arquivo, `broadcast` de `Arc<Bytes>` | portar (dá pra redesenhar em Python antes) |
| Lista de sessões (`registry.py`) | 2,2 ms com 3 sessões; ~0,5 ms de Python por sessão | 5–10× na parte Python; tmux/`/proc` iguais | portar por último |
| Estado/prévia (`state.py`, `preview.py`) | captura de pane 1,78 ms de CPU (Python só 0,18 ms) | só com redesenho: tmux em modo controle (`-C`) + tela virtual por sessão (`alacritty_terminal`, já usado no nativo): sessão parada custa zero | portar com redesenho |
| Terminal real (`termsock.py`, `conpty.py`) | já por evento | `portable-pty` unifica Linux e ConPTY | portar |
| Adaptadores (Claude/Codex/Kimi/Pi/omp) | 3,5 µs por evento; `_input_parcial` é quadrático (200 KB de tool input = 0,73 s de laço) | tipos gerados do schema do app-server do Codex (o Codex é Rust); enum de provider pega os 154 `provider == "x"` | `cano.py` já; resto por último (protocolo do Codex ainda `[experimental]`; 32% dos commits do backend são correção de CLI externa) |
| Custos/uso (`costs_*`, `uso_*`) | varredura fria 5,4 GB = 38 s em 1 núcleo (61% transformação Python); `/api/uso` sem cache 0,26–1,6 s | serde + rayon: 1–3 s (est.) | portar |
| Git/arquivos/cotas/mídia | git 2–8 ms (processo); HTTP remoto; ffmpeg | ~1 ms por chamada | portar só pelo binário único |

## Crescimento (custo medido × N)

| Cenário | Python hoje | Rust (est.) |
|---|---|---|
| 100 chats abertos | trava a API (200 tokens de thread) | ~10–30 MB a mais |
| 100 sessões headless | 2,2 GB e 600 threads só de `cano.py` | ~0,25 GB |
| 100 sessões trabalhando com tela capturada | ~670 processos/s, 1,2 núcleo | modo controle: ~0,1 núcleo, sessão parada = 0 |
| Board com 100 cards abrindo | 1–5 s de trabalho preso no GIL | 5–10× menos, espalhado nos núcleos |
| Histórico 10× (54 GB) | varredura fria ~6 min; `/api/uso` ~3 s | ~10–30 s; ~0,3 s |
| Windows (psmux) | ~50 ms por comando tmux; satura em ~12 sessões trabalhando | igual, a menos que psmux aceite `-C` (não verificado) |

O teto que pesa de verdade em qualquer linguagem são as próprias CLIs dos agentes: 400–790 MB
cada (100 sessões = 40–79 GB). O Hangar é ~5% disso.

## Como a migração do desktop ensina a fazer esta

- O desktop deu certo porque o backend ficou congelado como contrato e o Electron continuou
  instalado como reserva. O backend **é** o contrato e não pode rodar duas instâncias (mata as
  sessões headless). Por isso a migração não pode ser big-bang.
- Não há medição registrada Electron × nativo (`desktop-native/docs/verification.md`, passo
  13 pendente). Desta vez, gravar a linha de base antes.
- Ritmo do nativo: 62k linhas, 461 commits em 9 dias, 31% de correções.

## Caminho proposto (estrangulamento: o Rust assume rota a rota)

1. **Crate `hangar-api`** com os tipos do JSON (serde), usado pelo nativo já agora e gerando os
   tipos TS (ts-rs/specta). Paga mesmo se o resto não andar.
2. **Gravar respostas reais como fixtures douradas** (rotas, códigos 401/409/410/429, envelope
   `{"detail": …}`, nomes de evento SSE). Hoje não há teste de contrato.
3. **`cano.py` → binário Rust.** Isolado, protocolo por socket, maior ganho de memória por sessão.
4. **Porteiro Rust (axum) na 8765**, repassando ao Python (uma instância só, numa porta interna)
   tudo que ainda não foi portado. Primeiro a assumir: SSE de conversa + histórico + lista
   (os tetos 1 e 3).
5. Custos/uso, estado com modo controle + tela virtual, terminal real.
6. Adaptadores por último, gerados do schema; MCP com `rmcp` (conferir versão do spec) e push
   (crate `web-push` pouco mantido) fecham.
7. Sai o Python → binário único no instalador, igual ao `native-latest`.

Riscos: reproduzir formatos exatos sem teste de contrato (mitigado no passo 2); `rmcp` atrasado
no spec 2026-07-28; `web-push`; 88k linhas de teste em Python a reescrever; transcripts com
surrogates soltos (`models.py:22`) que o serde_json recusa.

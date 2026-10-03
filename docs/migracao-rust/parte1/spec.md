# hangar-server, parte 1 — conversas do Claude e do Codex em Rust, e o cano em Rust

**Data:** 2026-10-02
**Status:** Desenho aprovado na conversa; aguardando revisão desta spec
**Contexto:** primeiro subprojeto da migração do backend para Rust
([`docs/analise-backend-rust-2026-10-02.md`](../../analise-backend-rust-2026-10-02.md)).

## O que o dono pediu

- Migrar o backend inteiro para Rust **aos poucos**, cada parte em uso real antes da próxima,
  até o Python sumir e o instalador levar um binário só. Motivo: o projeto vai crescer (mais
  sessões, mais aparelhos, mais provedores), e o Python já tem teto medido.
- Esta primeira parte: o Rust assume **a leitura das conversas do Claude e do Codex** (histórico e
  chat ao vivo) e o **cano** é reescrito em Rust, em paralelo.
- Abordagem escolhida (A): o Rust lê a conversa sozinho; estado, prévia e perguntas continuam
  vindo do Python por **uma conexão interna por sessão**, e o Rust repassa a todos os aparelhos.
- Nome do processo: **`hangar-server`** (no fim ele é o servidor inteiro).
- Web, celular, nativo e peers **não mudam nada**: continuam falando com a `:8765`.

## Fora do escopo (subprojetos seguintes)

- Conversas do Pi, Kimi, omp e orq: o `hangar-server` repassa o streaming inteiro do Python.
- Lista de sessões, painel, quadro, custos, estado/prévia (tmux), terminal, adaptadores, envio de
  mensagens, contas, MCP, push, atualização: continuam no Python, por repasse.
- Porta de convidados (8766) e porta do Caddy (8768): continuam direto no Python.
- Convidado com login próprio na porta principal (`GuestUserGate`): toda requisição que não traz o
  token do dono é repassada ao Python, inclusive `/events` e `/history`.
- Gerar o `packages/core/src/types.ts` a partir do Rust: decidir quando mais formatos estiverem no
  crate.
- Inverter quem sobe quem (serviço subindo o Rust direto) e remover o Python: última etapa.

## Linha de base (medida em 2026-10-02, antes da troca)

Fonte: backend vivo (PID 403147, 3 sessões, 4 conexões) e benchmarks avulsos; detalhes na análise.

| Métrica | Valor |
|---|---|
| Memória do backend | 232–312 MB de RSS, 247 MB anônimos, 20 threads |
| CPU média | ~1,1% de um núcleo |
| `cano.py` por sessão sem terminal | 22 MB de RSS, 6 threads, 22 ms para subir |
| `/history` (Claude 0,9 MB / Codex 3 MB / Codex 35 MB) | 5–8 ms / 15–20 ms / 80–110 ms |
| `/history` completo, Claude 300 MB / `limit=200` | 362 ms / 45 ms |
| Atraso do laço com 8 leituras de histórico em paralelo | p99 9,6 ms, máx. 16 ms |
| Recursos presos por chat aberto | 2 threads do pool do anyio (limite 200, `api.py:301`) e 2 inotify |

Repetir as mesmas medidas depois da troca e anotar em `docs/decisoes/plataforma.md`.

## Como funciona

### 1. Quem sobe quem

- O serviço não muda: `hangar-backend.service` (Linux) e a tarefa `hangar-backend` (Windows)
  continuam rodando `python -m app.main`.
- `app/main.py` procura o binário, nesta ordem:
  1. `CP_RUST_SERVER_BIN`;
  2. `crates/target/release/hangar-server` do checkout (desenvolvimento);
  3. `~/.hangar/bin/hangar-server` (baixado).
- `CP_RUST_SERVER=0` desliga e o Python roda sozinho como hoje.
- Achou o binário: o uvicorn escuta a 8765 numa porta interna só de loopback (8766 e 8768 ficam
  como estão) e sobe o `hangar-server` como filho na porta pública (`CP_LAN_BIND_IP`:`CP_PORT`).
  - O filho recebe a porta interna, o token e um segredo interno novo a cada subida, que só ele e
    o Python conhecem.
  - O Python espera o `hangar-server` responder antes de se declarar de pé.
- **Sem binário, o binário não responde em 10 s, ou cai 3 vezes em 60 s:** o Python sobe um
  segundo `uvicorn.Server` no mesmo processo, na porta pública, e registra o motivo no diário
  (`log_paths.base()`). Até lá, cada queda religa o filho na hora.
- O filho morre junto com o Python: `PR_SET_PDEATHSIG` no Linux e job object no Windows.

### 2. O que o `hangar-server` atende sozinho

Só requisições com o **token do dono**, conferido do mesmo jeito que `auth.py:127`:

- ordem: `Authorization: Bearer`, depois `?token=`, depois cookie (`__Host-cp_token` em https,
  `cp_token` em http);
- cookie só vale em GET e HEAD;
- 8 falhas em 30 s por IP dá 429; loopback é isento.

Rotas que ele atende sozinho, para sessões cujo provider é `claude`, `claude-headless` ou `codex`:

- **`GET /api/sessions/{name}/history`**
  - Mesmos campos de `ChatEvent`, mesmo corte por `limit` com leitura de trás para frente em
    janelas (256 KB×4ⁿ), mesma junção da fila de pendentes de `pqueue.merged_history`
    (`pqueue.py:977`).
  - ETag próprio: o cliente só devolve o valor, e o formato não precisa casar com o do Python.
- **`GET /api/sessions/{name}/events`**, o chat ao vivo:
  - mesmos nomes de evento;
  - `id: <session_key>:<offset do início da linha>` só nas mensagens da conversa;
  - `?last_event_id=` vence o cabeçalho `Last-Event-ID`; offset maior que o arquivo volta para o
    fim;
  - sem id válido, manda as últimas 200 linhas;
  - evento `ping` na abertura e a cada 10 s, mais o comentário a cada 15 s;
  - cabeçalhos `Cache-Control: no-store` e `X-Accel-Buffering: no`; separador `\r\n`; sem
    `retry:`;
  - `?diag_req` e `x-hangar-req` vão para o log.
- CORS igual: `*`, sem credenciais, `ETag` exposto. Gzip só fora do `text/event-stream`, e só
  quando o cliente pedir.

Qualquer outra coisa é repassada ao Python, incluindo WebSocket, upload e streaming: `/mcp`, a
tela (`/`), terminal, rotas do Pi/Kimi/omp/orq e requisições sem o token do dono. O repasse
acrescenta `X-Forwarded-For` e `X-Forwarded-Proto`; o Python já confia em `127.0.0.1`
(`forwarded_allow_ips`).

### 3. A conexão interna (Python → `hangar-server`)

Rota nova no Python, `GET /internal/sessions/{name}/side-events`. Só aceita loopback com o segredo
interno e fica fora do catálogo da API. Uma por sessão, aberta enquanto houver ao menos um aparelho
naquele chat.

Ela emite, em ordem:

1. **`info`**: provider, caminho do `jsonl`, `session_key`. De novo a cada troca: as regras do
   `jsonl_watcher` (`sse.py:840`) continuam no Python — troca de provider na hora, troca de arquivo
   só depois de 2 leituras.
2. **Os eventos de fontes compartilhadas**, como hoje (`sse.py:955-976`): `state`, `stats`,
   `preview`, `pensamento`, `ferramenta`, `ask_question`, `suggest`, `nav`.
3. **Os eventos da fila** (`message`/`queue_confirmed` de `pqueue.follow`). O `hangar-server`
   guarda o último estado da fila por sessão para entregar a quem conecta depois.

Continuam no Python, dentro dessa conexão:

- digitar a fila quando a sessão fica livre (`adapter.drain`, `sse.py:1136`);
- a confirmação da fila do Codex (`_confirm_codex_queue`, `sse.py:661`), com o mesmo tail
  incremental;
- `plugin_bridge.app_entrou`/`app_saiu`. O `hangar-server` informa por `?app=1` quando há ao menos
  um aparelho do dono no chat.

Quando chega um `info` novo, o `hangar-server` emite `reset` para todos os aparelhos e recomeça a
leitura. Se o provider novo for um dos que ele ainda não lê, os aparelhos daquele chat passam a
receber o streaming do Python repassado: `reset` primeiro, e eles reconectam sozinhos.

Se a conexão interna cair, o `hangar-server` religa com espera crescente (1, 2, 4… até 30 s). Os
aparelhos continuam com o chat e recebem as mensagens da conversa, que vêm do arquivo.

### 4. Leitura da conversa no Rust

Formatos que precisam sair **byte a byte iguais** aos do Python, porque os clientes deduplicam por
eles e os ids não podem mudar na troca:

- **Claude** (`transcript.py:304,397`): `parse_line`/`parse_obj`, mais a supressão de reescritas do
  `RewriteFilter` (`transcript.py:315`). A impressão digital é o md5 da saída exata do
  `json.dumps` do Python (separadores `", "`/`": "`, `ensure_ascii=True`).
- **Codex** (`adapters/codex/rollout.py:222,336`): `parse_rollout_obj`/`parse_rollout_line`. O id
  de evento `_event_id` (`rollout.py:40`) é o sha1 do `json.dumps(sort_keys=True)` do Python;
  reproduzir a mesma serialização.
- **Surrogate solto** (`\ud8xx` sem par): o Python troca pelo mesmo resultado de `scrub_surrogates`
  (`models.py:22`). O `serde_json` recusa esse caso, então o Rust usa leitura tolerante na linha.
- **Linha que não entende:** pula, conta e registra no log do `hangar-server`, como o Python faz.
  Campo desconhecido é ignorado. O log nunca leva texto de conversa.
- **Arquivo:** um observador (`notify`) por pasta e um leitor por arquivo, compartilhados entre
  todos os aparelhos daquele chat. O mesmo frame já serializado vai para todos (`broadcast` de
  `Arc<Bytes>`). Arquivo truncado volta ao início com `reset`, como `TranscriptTailer`
  (`transcript.py:769`).

### 5. O cano em Rust (`hangar-cano`)

Substitui `backend/app/adapters/claude_headless/cano.py` (424 linhas) com o mesmo contrato, para o
Claude e para o Codex sem terminal (`codex/sem_terminal.py:115`).

- **Mesma linha de comando:** `--escuta unix:<caminho>|tcp:127.0.0.1:<porta>`, `--log`, `--cwd`,
  `--token`, `-- <argv>`. Socket `0600`, backlog 2, bind antes de subir o filho. Saída 1 em falha
  de bind/spawn, 2 sem argv.
- **Mesmo protocolo** (linhas UTF-8):
  - token na primeira linha, com prazo de 10 s;
  - `cano_snapshot` com `versao: 1` e os mesmos campos;
  - `cano_stderr`;
  - `cano_saiu` síncrono com prazo de 5 s;
  - um cliente por vez, e o novo derruba o antigo;
  - fila de 5.000 linhas, descartando e avisando uma vez no log;
  - rastreamento de `init`/`aberto`/`pendentes`/`ultimo_result`/`rate_limit` igual a
    `cano.py:103-158`.
- **Mesmo ciclo de vida:**
  - quando o filho sai, espera até 60 s alguém receber o código, apaga o socket e sai 0;
  - SIGTERM encerra o filho, apaga o socket e sai.
  - No Windows: TCP com token e stderr na codepage local com reserva cp1252
    (`cano.py:326-341`).
- **Escolha no backend** (`adapter.py:2144`): binário achado (mesma ordem de busca do item 1, com
  `CP_RUST_CANO_BIN`) → `hangar-cano`; senão `cano.py`.
- **Ficam iguais:**
  - `systemd-run --scope`;
  - o grupo de processos;
  - o `HANGAR_CANO_KEY` no ambiente;
  - a chave da sessão no `cmdline`, de que `registry.cwd_atual` depende (`registry.py:169`).
- Sessões já abertas seguem no `cano.py` até reabrir: o snapshot `versao: 1` é o mesmo, então não
  há reabertura forçada.

### 6. Código e distribuição

- **`crates/` vira um workspace Cargo próprio:**
  - `hangar-api`: os tipos da conversa (`ChatEvent`, estado, prévia, pergunta, estatísticas), com
    `Serialize` e `Deserialize` tolerantes (`#[serde(default)]`, sem `deny_unknown_fields`);
  - `hangar-server`: axum/tokio;
  - `hangar-cano`: tokio `current_thread`.
- Toolchain 1.98.1 e versões fixadas com `=`, como no `desktop-native`.
- O `desktop-native` passa a depender de `../crates/hangar-api` e troca as structs equivalentes do
  `src/api/dto.rs` pelas do crate.
- **CI:**
  - workflow novo `server.yml`: `cargo test` e build release de `crates/` em Linux, Windows e macOS;
    publica `hangar-server` e `hangar-cano` na release fixa `server-latest` com manifesto sha256,
    como o `native-latest`;
  - o `native.yml` passa a disparar também com mudança em `crates/hangar-api/**`.
- **Instalação e atualização:** o `install.sh`/`install.ps1` e o `atualizar.py` baixam os dois
  binários da máquina para `~/.hangar/bin/` e conferem o sha256. Falha no download não trava nada:
  o Python segue sozinho e o diário registra. O serviço e a tarefa do Windows não mudam. Antes de
  mexer, ler as "Regras vigentes" de `docs/decisoes/instalacao.md` e `windows.md`.

## Como provar

- **Contrato da conversa:**
  - Transcripts de exemplo em `backend/tests/fixtures/` (Claude e Codex), cobrindo: surrogate
    solto, mensagem reescrita, tool use/result, fila pendente, arquivo truncado e linha inválida.
  - Um script Python gera o resultado esperado com os parsers atuais: `/history` com e sem `limit`
    e a sequência de eventos ao vivo, com ids.
  - Os testes do `hangar-server` comparam campo a campo com esse resultado.
  - Nunca usar conversa real em fixture.
- **Cano:** `tests/test_claude_headless_cano.py` parametrizado para rodar contra `cano.py` e contra
  `hangar-cano`. O teste de codepage (`_texto_do_stderr`) ganha um equivalente em teste Rust.
- **Queda e reserva:** teste do `main.py` em que o binário falso cai 3 vezes e o Python assume a
  porta pública.
- **Uso real, com o dono, no fim:**
  - abrir chats do Claude e do Codex no celular, no web e no nativo;
  - derrubar a rede e reconectar sem perder nem duplicar mensagem;
  - reiniciar o backend com uma sessão sem terminal viva e conferir que ela continua;
  - `CP_RUST_SERVER=0` volta ao Python sozinho;
  - repetir as medidas da linha de base.
- Pela regra do projeto, os testes automatizados só rodam quando o dono pedir.

## Pronto quando

- `/history` e `/events` do Claude e do Codex saem do `hangar-server`, com o contrato passando e os
  três clientes sem mudança.
- O teto de chats abertos deixa de depender do pool de threads do Python: uma conexão interna por
  sessão, e não por aparelho.
- Sessões novas sem terminal rodam no `hangar-cano`, com os testes do cano passando nas duas
  implementações e a memória por sessão medida abaixo dos 22 MB de hoje.
- Sem binário, com `CP_RUST_SERVER=0` ou com o binário caindo, tudo funciona como hoje.
- As medidas de antes e depois estão anotadas em `docs/decisoes/plataforma.md`, junto com a regra
  nova no `CLAUDE.md`.

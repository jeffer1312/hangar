# Dividir o `hangar-server` em vários crates

**Objetivo.** Tirar o `hangar-server` de um crate só de ~251 mil linhas, cujo `rustc` chega a
~9,3 GB, para um workspace em que nenhum crate depende do `hangar-server` e cada recurso compila
sozinho. Este plano cobre a primeira fase: o `hangar-core`, o fim das ligações que prendem tudo ao
`AppState` e ao par `runtime` ↔ `accounts`, e os primeiros crates de recurso. A segunda fase (o nó
`runtime`/`accounts`/`list`/`session_open`/`session_write`/`state`) é planejada na Task 10, com o
grafo daquele momento.

**Fonte dos requisitos.**

- Pedido do Jefferson, 06/11/2026: etapas (a) criar o `hangar-core` e descer o que não depende de
  ninguém; (b) desfazer o `AppState` e o ciclo `runtime` ↔ `accounts` com trait no core; (c) tirar
  um crate por vez, começando pelos módulos soltos. Cada etapa é um PR que compila, passa nos
  testes e deixa o servidor funcionando.
- Regra "Módulo Rust novo nasce como crate próprio quando não precisa do `hangar-server`"
  (`CLAUDE.md`, seção Plataforma) e a entrada
  [Módulo novo nasce como crate próprio](../../decisoes/plataforma.md#módulo-novo-nasce-como-crate-próprio-06112026).
- Skills `rust-crate-architecture` (camadas, Fix A = descer o tipo, Fix B = inverter com trait,
  divisão sem diff ilegível), `rust-workspace` e `rust-code-style` (visibilidade).

**Restrições.** Nenhuma Task muda comportamento: é movimento de código e inversão de dependência.
Compilar e testar (`cargo`, `scripts/cargo-pinned`, `scripts/verificar-local`) só quando o
Jefferson pedir; os comandos de prova abaixo valem nesse momento. Push e PR, só com autorização.

## Números conferidos

Grafo refeito em `0452a7f55` (origin/main de 06/11/2026), por análise estática do texto, sem
compilar. Aresta Y→X = cada `crate::X`, `crate::{X, …}`, `super::X` que sai do módulo, e `X::` no
corpo do `lib.rs`.

| Medida | No pedido | Refeito | Por que difere |
|---|---|---|---|
| Linhas do crate | 250.599 | 251.096 (250.410 nos módulos + 463 `lib.rs` + 223 `main.rs`) | 44 commits a mais |
| Módulos de topo | 123 | 123 | — |
| No ciclo | 108 módulos, 247.669 linhas | 111 módulos + corpo do `lib.rs`, 248.681 linhas | Contei `crate::{…}`, `super::` e o `lib.rs` (que usa `routes::AppState` em `serve_until` e é usado por 27 módulos: `AbortOnDrop`, `nodelay`, `http_client`). Na `main` antiga o mesmo método dá 110: a diferença é o método |
| Fora de ciclo | 15 módulos, 2.930 linhas | 11 módulos, 1.729 linhas | Idem. Os 11: `orq_storage`, `terminal_state`, `plugin_listener`, `value_coercion`, `json_envelope`, `terminal_lock`, `warn_limit`, `costs_failure`, `http_contract`, `http_route_response`, `http_body_limit` |
| `routes::AppState` | 124 usos | o texto `routes::AppState` aparece 154 vezes; `AppState` aparece 811 vezes fora de `routes`, em 72 módulos | contagem de outra coisa (provável: linhas de `use`) |
| `routes` usa | todos os módulos | 93 dos 122 | — |
| Portões de `routes` | — | `cors` e `gate` em 36 módulos, `pass` em 29, `owner_gate` em 11, `on_guest_port` em 8 | — |
| `auth` | 448 linhas, 56 módulos | 448 linhas, 58 módulos (52 sem código de teste) | método |
| `runtime` ↔ `accounts` | `accounts`→`runtime` 33 | 33 (31 sem teste); `runtime`→`accounts` 52 (42 sem teste) | — |
| Maiores | runtime 27.097 … orchestration 6.468 | runtime 27.136, accounts 17.877, voice 12.717, list 12.045, update 11.735, session_open 11.586, speech 7.871, codex_setup 7.290, groups 6.872, orchestration 6.468 | 44 commits |

Fatos novos que mudam o plano:

- **O ciclo não vem dos testes.** Tirando `#[cfg(test)]` e os arquivos `tests.rs`/`*_tests.rs`,
  o ciclo é o mesmo (111 módulos + `lib.rs`).
- **Quem prende os módulos médios são ajudantes que moram dentro dos grandes.** Usos de fora:
  `runtime::queue::atomic_write` 75, `orchestration::truthy` 58, `session_write::json_response` 53,
  `runtime::gateway::RuntimeRegistry` 50, `orchestration::codec::discard` 45,
  `runtime::queue::read_replaced` 35, `list::procs::SystemProcs` 23, `runtime::protocol::RuntimeError` 22,
  `list::links::without_verbatim` 21, `list::discover_other::sanitize_session_name` 21,
  `workspace_routes::private_ok` 19, `state::monitor::wall_now` 14.
- **Mesmo com esses ajudantes no core, sobra um nó de 84 módulos (~220 mil linhas).** `runtime`,
  `accounts`, `list`, `session_open`, `session_write` e `state` dependem de verdade um do outro. A
  primeira fase não o desfaz: é o objeto da Task 10.
- **Os primeiros crates de recurso, pelo que falta cortar** (alvos fora do core, depois das Tasks
  1–3): `costs` 1 alvo (`transcript::py`, que desce na Task 2) — fica livre; `transcript` 2 alvos/3
  referências; `voice` 3 alvos/4 referências (`groups::peers`, `accounts::AccountService`,
  `share::gate::preflight`); `omp_sync` 4/7; `mods` 5/12; `remote_browser` 4/12; `speech` 6/21;
  `orchestration` 7/22.

O script da análise ficou fora do repositório. As provas das Tasks não dependem dele: o compilador
recusa referência ao `hangar-server` dentro de um crate novo, e `cargo metadata` mostra as arestas
entre crates.

## Arquitetura alvo

| Camada | Crates | Pode depender de |
|---|---|---|
| Base (existentes) | `hangar-api`, `hangar-auth`, `hangar-codex`, `hangar-workspace`, `hangar-package` | crates externos e entre si |
| Core | `hangar-core` (novo): ajudantes e tipos comuns do servidor, com I/O de arquivo e HTTP; traits das inversões | base |
| Recursos | `hangar-costs`, `hangar-voice`, `hangar-transcript`, `hangar-omp-sync`… | core, base e recursos mais baixos |
| Topo | `hangar-server`: `AppState`, montagem das rotas, `main.rs` e o que ainda não saiu | todos |

Invariante: nenhuma aresta `[normal]` ou `[build]` aponta para o `hangar-server`. Conferência:

```bash
cd crates && cargo metadata --locked --no-deps --format-version 1 \
  | jq -r '.packages[] | .name as $n | .dependencies[] | select(.path != null)
           | "\($n) -> \(.name) [\(.kind // "normal")]"' | sort | grep -- '-> hangar-server'
# esperado: nenhuma linha
```

## Contrato de cada extração

Vale para toda Task que move código para outro crate.

1. **O caminho antigo continua valendo.** O `lib.rs` do `hangar-server` troca `pub mod x;` por
   `pub use hangar_<crate>::x;` (e `pub(crate) use` para os itens de topo, como `nodelay`). Assim
   `crate::x::…` segue compilando no servidor, nos testes `tests/it` (`hangar_server::x::…`) e nas
   branches das sessões que trabalham em paralelo. Os re-exports só saem num PR próprio, depois da
   segunda fase.
2. **Movimento sem edição.** `git mv` dos arquivos; no mesmo PR, só o necessário para compilar:
   manifesto, re-export, `crate::` interno que passa a apontar para o crate novo, e visibilidade.
   Corte de ligação para cima (Fix A/Fix B) vai num commit anterior, que compila ainda dentro do
   servidor.
3. **Visibilidade.** Item `pub(crate)` usado pelo servidor vira `pub`; revisar cada um, pois vira
   API do crate. Item que só o próprio crate usa continua `pub(crate)`. Nada de `pub` em massa.
4. **Impl com o tipo.** Impl inerente e impl de trait estrangeira (axum, serde, std) vão junto com o
   tipo, mesmo em outro arquivo (senão E0116/E0117). Impl de trait do servidor para tipo movido pode
   ficar no servidor.
5. **Testes.** Teste inline que usa só o código movido vai junto. Teste que usa o servidor vai para
   `crates/hangar-server/tests/it/` (pela API pública) ou para um módulo `#[cfg(test)]` do
   servidor. Ajudante de teste usado pelos dois lados (`write_test_executable`,
   `voice::test_support`) fica atrás de uma feature `test-support` do crate novo, ligada só em
   `[dev-dependencies]` do servidor.
6. **Manifesto.** `version.workspace = true`, `edition.workspace = true`, `publish = false`;
   entrada em `members` e em `[workspace.dependencies]` de `crates/Cargo.toml`; dependências pelo
   workspace; as mesmas features de `serde`, `serde_json` e `tokio` que o
   `crates/hangar-server/Cargo.toml` declara (o comentário de lá explica a cópia duplicada no
   `target`). `#![forbid(unsafe_code)]` quando o código movido não tem `unsafe`. Dependência
   externa que só o código movido usava sai do manifesto do servidor. `Cargo.lock` no mesmo commit.
7. **Fica no servidor:** `INTERNAL_PROTOCOL` no `lib.rs` (o `server-jobs.yml` o lê de lá), o
   `AppState`, a montagem das rotas e o `main.rs`.
8. **Portões já cobrem os crates novos.** CI (`server-jobs.yml`) e `scripts/verificar-local`
   rodam `cargo test --locked --workspace`, e qualquer arquivo em `crates/` liga o passo `rust`.
   Nenhuma mudança neles é necessária, salvo o que a Task disser.

Provas comuns, rodadas quando o Jefferson pedir, nesta ordem e uma por vez (`cargo-pinned` segura a
trava da máquina):

```bash
scripts/cargo-pinned build -p hangar-server                       # compila; servidor e crate novo
scripts/cargo-pinned test -p <crate-novo>                          # testes do crate novo
scripts/cargo-pinned test -p hangar-server --test it               # integração do servidor
scripts/verificar-local                                            # antes do PR
```

E a conferência da invariante acima (nenhuma aresta para `hangar-server`).

## Coordenação com as sessões que migram Python → `hangar-server`

Cada Task move arquivos que outras sessões podem estar editando. O re-export do item 1 do contrato
mantém o código delas compilando; o risco é conflito de merge no arquivo movido e no `lib.rs`.

**Decidido (Jefferson, 06/11/2026): janela por Task.** As outras opções eram esperar nenhuma branch
mexer em `crates/hangar-server/` (dias de espera, a migração é contínua) e seguir sem aviso (o
conflito cairia em quem não planejou a divisão).

Em toda Task que move arquivo:

1. **Antes de mover**, listar as sessões vivas do repositório (`hangar-send --list`, as de `cwd` num
   checkout do hangar) e mandar a cada uma, por `hangar-send <sessao> "…"`, a lista exata de
   arquivos e módulos que vão mudar de lugar e o pedido de não editá-los até o aviso de merge.
   Sessão que já tem mudança neles responde, e a Task espera ela mesclar ou guardar a mudança.
2. **Depois do merge na `main`**, avisar as mesmas sessões que os arquivos estão livres, com o
   caminho novo de cada um e o lembrete de atualizar a branch sobre o `origin/main`.
3. O caminho antigo (`crate::modulo::…`) continua compilando pelo re-export; quem só usa o módulo
   não precisa parar.

## Estimativa de memória

Estimativa, não medida. Premissa: o pico do `rustc` cresce com as linhas do crate. Base: 9,3 GB para
~251 mil linhas, ≈ 37 MB por mil linhas, mais ~0,5 GB fixos de metadados das dependências (axum,
tokio, reqwest, rmcp). A premissa erra para cima em crate pequeno e para baixo no crate que monta
o roteador do axum (monomorfização concentrada nele).

| Momento | Maior crate | Pico estimado por `rustc` | Lib + `it` em paralelo |
|---|---|---|---|
| Hoje (medido) | `hangar-server`, ~251 mil | 9,3 GB | ~18 GB |
| Depois desta fase (Tasks 1–9; saem ~33 mil linhas) | `hangar-server`, ~218 mil | ~8,6 GB | ~17 GB |
| Crates de recurso desta fase | `hangar-voice`, ~12,7 mil | ~1 GB | ~2 GB |
| Meta da segunda fase (servidor só como topo, < 60 mil) | — | ~2,7 GB | ~5,5 GB |

Leitura honesta: esta fase prova o caminho e para o crescimento do servidor (regra no
`CLAUDE.md`), mas reduz pouco o pico. O ganho real vem de desfazer o nó de 84 módulos na segunda
fase.

## Ordem e paralelismo

As Tasks são uma cadeia: cada uma usa o core da anterior e move arquivos que a próxima toca.
Nenhuma roda em paralelo com outra. Tasks de base ou risco, com revisão: 1 (contrato de manifesto
que todas seguem), 3 (autorização do dono), 4 (padrão de estado que todas seguem), 5 (inversão
entre `runtime` e `accounts`).

Os módulos soltos do pedido: dos 11 medidos, 9 entram no core na Task 1. `plugin_listener` (114
linhas, usado só por `routes`) e `orq_storage` (627, usado só por `main.rs`) não ganham crate
próprio: o ganho de memória é nulo e crate com uma responsabilidade tão pequena não se paga
(`rust-crate-architecture`, "Crate or module?"). `orq_storage` vai com `orchestration` na segunda
fase; `plugin_listener` fica no servidor. Depois do core, a ordem segue o que falta cortar (seção
"Números conferidos").

### Task 1: Criar o `hangar-core` com os módulos que não dependem de ninguém

Status: ready-for-agent
Revisão: base — define o manifesto e o re-export que todas as Tasks repetem.

Arquivos: `crates/hangar-core/{Cargo.toml,src/lib.rs}` (novos); `git mv` de
`crates/hangar-server/src/{warn_limit,http_contract,http_route_response,http_body_limit,json_envelope,value_coercion,terminal_state,terminal_lock,costs_failure}.rs`
para `crates/hangar-core/src/`; `crates/Cargo.toml`; `crates/hangar-server/{Cargo.toml,src/lib.rs}`;
`crates/Cargo.lock`.

Do corpo do `lib.rs` descem `http_client()`, `nodelay()` e `AbortOnDrop` (módulo `net` do core),
usados por 27 módulos como `crate::http_client`, `crate::nodelay` e `crate::AbortOnDrop`; o
`lib.rs` os re-exporta com a mesma visibilidade. `write_test_executable` desce atrás da feature
`test-support`.

- [ ] **Step 1: Conferir que os nove módulos e os três itens do `lib.rs` não usam nada do servidor**
  `grep -nE 'crate::|super::' <arquivos>` → só `http_route_response` → `http_contract` (que desce
  junto). Qualquer outra referência: parar e trazer para o Jefferson.
- [ ] **Step 2: Criar o crate e mover os arquivos** conforme os itens 2 e 6 do contrato.
- [ ] **Step 3: Ajustar visibilidade e re-exports** (itens 1 e 3 do contrato).
- [ ] **Step 4: Provas comuns** (quando autorizadas) e a invariante sem aresta para o servidor.
- [ ] **Step 5: Uso real — verificação manual** `systemctl --user restart hangar-backend.service`
  com o binário novo; abrir o app, ver a lista de sessões e mandar uma mensagem numa sessão.

### Task 2: Descer `auth`, `config` e os ajudantes de resposta HTTP e JSON (Fix A)

Status: ready-for-agent (depois da Task 1)

Corta as ligações para cima dos módulos-base. Cada função listada desce inteira, com os testes
dela, para o módulo do core indicado; o lugar antigo fica com `pub use` até a limpeza final.

| O quê | De | Para (`hangar-core`) |
|---|---|---|
| `json_response`, `detail`, `detail_body`, `too_large`, `BODY_LIMIT`, `busy_body` (+ `BUSY_MSG`) | `session_write/mod.rs` | `http::response` |
| `py`, `pyjson`, `json_tree`, `json_writer` | `transcript/` | `pycompat` |
| `load`, `py_str` (e o que só elas usam) | `orq_conductor/mod.rs` | `pycompat` |
| `codec`, `truthy` (`common.rs`), `body::parse` | `orchestration/` | `pycompat` / `http::body` |
| módulos inteiros `auth`, `config`, `query`, `http_cors`, `http_body_input` | topo | mesmos nomes |

- [ ] **Step 1: Listar as dependências de cada item** e confirmar que, depois da tabela inteira,
  nenhum aponta para o servidor (`codec` usa `orq_conductor::{load,py_str}` e `transcript::pyjson`,
  que descem juntos; `auth` usa `session_write::json_response`; `config` usa `auth`).
- [ ] **Step 2: Commit de corte** com as funções movidas para o core e `pub use` nos lugares antigos.
- [ ] **Step 3: Commit de movimento** dos cinco módulos inteiros, com re-export no `lib.rs`.
- [ ] **Step 4: Provas comuns** (quando autorizadas).
- [ ] **Step 5: Uso real — verificação manual** login do dono e de convidado (porta 8766) e uma
  chamada que responde erro JSON (ex.: sessão inexistente devolve o `detail` com `code`).

### Task 3: Portão do dono e repasse ao Python no core

Status: ready-for-agent (depois da Task 2)
Revisão: risco — autorização do dono e do convidado.

Os módulos de recurso usam `routes::{gate, owner_gate, who, forward_for, on_guest_port, pass, cors,
not_found, maybe_gzip, route_failed}` recebendo o `AppState` inteiro, mas o portão só lê
`cfg.trusted`, `auth`, `http`, `cfg.upstream`, `cfg.internal_secret` e marcas nas extensões do
pedido. Esses campos viram uma struct do core:

```rust
pub struct Edge { pub cfg: Config, pub auth: Auth, pub http: HttpClient }
impl Edge {
    pub fn gate(&self, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<(Forward, bool), AuthError>> + Send + use<>;
    pub fn owner_gate(&self, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<Forward, Response>> + Send + use<>;
    pub async fn pass(&self, req: Request, fwd: &Forward) -> Response;
    // who, forward_for: idem
}
```

Descem junto: `Forward`, `RequestAuth`, `proxy` (cliente, `forward`, `AUTH_EMPTY_QUERY`), as marcas
`session_write::bridge::BridgedOwner`, `share::gate::GuestConn`, `login_guests::gate::Login`, o
`Caller`/`GuestGrant` e `share::forward::{prepare_guest_request, HEADER, pipe_watched}`;
`cors`, `not_found`, `maybe_gzip`, `gzip_with_minimum`, `route_failed`, `on_guest_port` e
`share::gate::preflight` viram funções livres do core. O `AppState` ganha `edge: Arc<Edge>`
(montado em `AppState::new`), e as funções de `routes` passam a delegar a ele, com a mesma
assinatura: nenhum chamador muda nesta Task.

- [ ] **Step 1: Inventariar os campos que o portão lê** (`routes.rs` de `gate` a `pass`, e
  `share::gate`) e conferir que nenhum outro campo do `AppState` entra.
- [ ] **Step 2: Descer tipos e marcas** (Fix A), com `pub use` nos lugares antigos.
- [ ] **Step 3: Criar `Edge` e delegar** as funções de `routes` a ele.
- [ ] **Step 4: Provas comuns** (quando autorizadas), com atenção aos testes de autorização em
  `tests/it` (dono, convidado, bloqueio por tentativas, recibo de admissão).
- [ ] **Step 5: Uso real — verificação manual** dono pelo token, dono pelo cookie, convidado por
  convite na 8766 e token errado recusado com 401 e depois 429.

### Task 4: Estado por recurso no lugar do `AppState` — piloto em `costs_routes`

Status: ready-for-agent (depois da Task 3)
Revisão: base — define o padrão que cada crate de recurso repete.

O `AppState` continua sendo a raiz que o servidor monta; o que acaba é recurso depender dele.
Cada recurso declara o estado que usa e o servidor entrega por `axum::extract::FromRef`:

```rust
// no recurso
#[derive(Clone)]
pub struct CostsState { pub edge: Arc<Edge>, pub costs: Arc<Collector>, pub fx: Arc<Fx>, pub reports: Arc<ReportCache> /* … */ }
pub fn router() -> Router<CostsState>;

// no hangar-server
impl FromRef<Arc<AppState>> for CostsState { fn from_ref(st: &Arc<AppState>) -> Self { /* clona os Arc */ } }
```

Piloto em `costs_routes` (433 linhas; usa `routes::{AppState, cors, gate, maybe_gzip, pass,
route_failed}`), ainda dentro do servidor: a Task prova o padrão sem mover arquivo.

- [ ] **Step 1: Listar os campos do `AppState` que `costs_routes` lê** e montar `CostsState` só com eles.
- [ ] **Step 2: Trocar `State<Arc<AppState>>` por `State<CostsState>`** e `routes::gate(st, …)`
  por `st.edge.gate(…)`; registrar o `FromRef` no servidor.
- [ ] **Step 3: Provas comuns** (quando autorizadas); `grep -n AppState crates/hangar-server/src/costs_routes.rs` → vazio.
- [ ] **Step 4: Uso real — verificação manual** abrir Custos no app e conferir que os números
  batem com os de antes da troca.

### Task 5: Desfazer o ciclo `runtime` ↔ `accounts`

Status: ready-for-agent (depois da Task 4)
Revisão: risco — contrato entre os dois maiores módulos.

Do lado de `accounts`, as 33 referências a `runtime` são de dois tipos.

Fix A (descem para o core): `runtime::queue::{atomic_write, replace, read_replaced,
read_replaced_to_string}` → `hangar_core::fs`; `runtime::process::{terminate, scope_prefix,
resolve_program}` → `hangar_core::process`; `runtime::local_policy::{log_base, log_base_in}` →
`hangar_core::paths`. Com eles, desce `accounts::environment::AccountEnvironment` (usado por 15
lugares de `runtime`, por `diag`, `install_root` e `user_data`), depois de conferir o que ele usa
de `accounts::catalog` e `accounts::secondary_auth`. Com isso descem inteiros para o core os
módulos `diag`, `user_data`, `install_root` e `terminal_process`, levando as funções de que ainda
precisam: `session_open::cli::{pointer, is_backend}`, `list::links::without_verbatim` e
`list::procs::vanished`.

Fix B (trait no core): `accounts` guarda `Option<Arc<RuntimeRegistry>>` em `claude_refresh`,
`codex_login`, `codex_device_login`, `quotas` e `usage` (17 citações), mas chama poucos métodos
(ex.: `account_usage` em `usage.rs:356`; os de `device_guard` e o `refresh_hooks` de
`accounts/http.rs:552`). Um trait `AccountRuntime` no core, com só esses métodos,
implementado pelo `RuntimeRegistry`; `accounts` passa a guardar `Arc<dyn AccountRuntime>`.

- [ ] **Step 1: Inventariar os métodos do `RuntimeRegistry` que `accounts` chama** (grep por campo
  e por parâmetro) e escrever o trait com exatamente esses.
- [ ] **Step 2: Commit Fix A** com as funções e `AccountEnvironment` no core e `pub use` nos lugares antigos.
- [ ] **Step 3: Commit Fix B** com o trait, a implementação no `runtime` e a troca dos campos em `accounts`.
- [ ] **Step 4: Conferir o corte:** `grep -rn 'crate::runtime' crates/hangar-server/src/accounts`
  fora de teste → vazio.
- [ ] **Step 5: Provas comuns** (quando autorizadas).
- [ ] **Step 6: Uso real — verificação manual** cota das contas Claude e Codex aparece; login
  Codex por aparelho conclui; sessão que bate limite troca de conta.

### Task 6: Extrair `hangar-costs`

Status: ready-for-agent (depois da Task 5)

`costs` (5.121 linhas) só dependia de `transcript::py`, `proxy::client` e `warn_limit`, que já
estão no core; `costs_routes` já usa `CostsState` (Task 4).

- [ ] **Step 1: Conferir** `grep -rnE 'crate::' crates/hangar-server/src/{costs,costs_routes.rs}` →
  só `crate::costs`, `crate::costs_routes` e itens re-exportados do core.
- [ ] **Step 2: Mover `costs/` e `costs_routes.rs`** para `crates/hangar-costs/` pelo contrato.
- [ ] **Step 3: Provas comuns** (quando autorizadas), incluindo `tests/contract_*` que citam `hangar_server::costs` (31 usos).
- [ ] **Step 4: Uso real — verificação manual** Custos e Uso no app.

### Task 7: Extrair `hangar-voice`

Status: ready-for-agent (depois da Task 6)

`voice` (12.717 linhas) usa do servidor: `groups::peers` (`PeerClient`, `PeerBook`, `PeerError`,
`TOO_LARGE`), `accounts::AccountService::visible_codex_accounts`, `share::gate::preflight` (já no
core pela Task 3), `display_env` e `terminal_process::PRIVATE_ENV_KEYS`.

- `display_env` (168 linhas) só serve a voz e loga por `voice::log`: vai para dentro do crate.
- `terminal_process::PRIVATE_ENV_KEYS` já está no core (Task 5).
- `groups::peers` (409 linhas) usa `diag::record_blocking`, `mods::state::random_hex`,
  `transcript::py`, `warn_limit` e `http_client`: desce para o core com `random_hex` (Fix A), já
  que `diag` desceu na Task 5.
- `AccountService`: trait `CodexAccounts { fn visible_codex_accounts(&self) -> … }` no crate da voz,
  implementado no servidor (Fix B).
- O `VoiceHub` passa a receber o estado pelo padrão da Task 4.

- [ ] **Step 1: Conferir as referências** `grep -rnE 'crate::' crates/hangar-server/src/voice` fora de `crate::voice`.
- [ ] **Step 2: Commit de corte** (Fix A e Fix B acima).
- [ ] **Step 3: Mover `voice/` e `display_env.rs`** para `crates/hangar-voice/`; `test_support` atrás da feature `test-support`.
- [ ] **Step 4: Provas comuns** (quando autorizadas).
- [ ] **Step 5: Uso real — verificação manual** chamada de voz no nativo: falar, ouvir a resposta,
  trocar de sessão por voz.

### Task 8: Extrair `hangar-transcript`

Status: ready-for-agent (depois da Task 7)

`transcript` (3.665 linhas) é lido por 45 módulos; depois da Task 2 restam duas ligações para
cima: `orchestration::timeline::parse_obj` (em `transcript/history.rs:399` e `transcript/mod.rs:131`,
para `Provider::Orq`) e `runtime::queue::row_waits` (`history.rs:584`).

- `row_waits` (`runtime/queue.rs:566`) é função pura sobre `Value`: desce para o core (Fix A).
- `parse_obj` é comportamento da orquestração: o leitor do transcript recebe um
  `Arc<dyn OrqTimeline>` (trait no crate do transcript) que o servidor implementa chamando
  `orchestration::timeline::parse_obj` (Fix B).

- [ ] **Step 1: Conferir as referências** de `transcript/` fora de `crate::transcript` e do core.
- [ ] **Step 2: Commit de corte** (`row_waits` e o trait `OrqTimeline`).
- [ ] **Step 3: Mover `transcript/`** para `crates/hangar-transcript/` pelo contrato.
- [ ] **Step 4: Provas comuns** (quando autorizadas).
- [ ] **Step 5: Uso real — verificação manual** abrir conversas Claude, Codex e uma sessão `orq`;
  histórico e eventos ao vivo aparecem.

### Task 9: Extrair `hangar-omp-sync`

Status: ready-for-agent (depois da Task 8)

`omp_sync` (5.003 linhas) usa: `accounts::secondary_auth::{omp_directories, OmpDirectories}`,
`install_root::root` (core pela Task 5), `loops::automations`, `nav_requests::now`,
`push::server_env`, `runtime::process::resolve_program` e `runtime::queue::atomic_write` (core pela
Task 5), `session_write::json_response` e `transcript::pyjson` (core pela Task 2) e
`routes::AppState` (padrão da Task 4).

- [ ] **Step 1: Decidir o corte de cada uma das restantes** (`omp_directories`, `automations`,
  `now`, `server_env`): Fix A se for função pura ou leitura de caminho; Fix B se for estado do servidor.
- [ ] **Step 2: Commit de corte.**
- [ ] **Step 3: Mover `omp_sync/`** para `crates/hangar-omp-sync/`.
- [ ] **Step 4: Provas comuns** (quando autorizadas).
- [ ] **Step 5: Uso real — verificação manual** sincronização omp: plugins e contexto reconciliados numa sessão omp.

### Task 10: Refazer o grafo e planejar a segunda fase

Status: ready-for-agent (depois da Task 9)

- [ ] **Step 1: Refazer o grafo de módulos** do que sobrou no `hangar-server`, com o mesmo método
  da seção "Números conferidos", e a lista de alvos restantes por módulo.
- [ ] **Step 2: Medir a memória de um `rustc` do `hangar-server`** e de cada crate novo (quando o
  Jefferson autorizar o build), e corrigir a tabela de estimativa com o medido.
- [ ] **Step 3: Escrever o plano da segunda fase** em `docs/superpowers/plans/`: `speech`,
  `orchestration` (com `orq_storage`), `remote_browser`, `mods`, e o nó `runtime`/`accounts`/`list`/
  `session_open`/`session_write`/`state`; incluir o PR que retira os re-exports do item 1 do contrato.
- [ ] **Step 4: Atualizar a entrada de plataforma.md** com os números medidos.

## Aceitação desta fase

- `hangar-core`, `hangar-costs`, `hangar-voice`, `hangar-transcript` e `hangar-omp-sync` existem;
  nenhum depende do `hangar-server` (conferência por `cargo metadata`).
- `grep -rn 'crate::runtime' crates/hangar-server/src/accounts` sem código de teste → vazio.
- `costs_routes` e os crates extraídos não citam `AppState`.
- Cada Task foi um PR com as provas comuns verdes e o uso real conferido.
- A memória medida na Task 10 substitui a estimativa.

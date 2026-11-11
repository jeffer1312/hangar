# Dividir o `hangar-server` em vários crates

**Objetivo.** Tirar o `hangar-server` de um crate só de ~251 mil linhas, cujo `rustc` chega a
~9,3 GB, para um workspace em que nenhum crate depende do `hangar-server` e cada recurso compila
sozinho. Este plano cobre a primeira fase: o `hangar-core`, o fim da dependência dos recursos no
`AppState`, a descida dos ajudantes que prendem `accounts` ao `runtime`, e os primeiros crates de
recurso. A segunda fase (o nó `runtime`/`accounts`/`list`/`session_open`/`session_write`/`state`)
é planejada na Task 10, com o grafo daquele momento.

**Revisão.** O plano passou por uma crítica adversarial (agente arquiteto) e por uma conferência
de cada símbolo citado, em 06/11/2026; as correções já estão no texto (ordem das Tasks 3 e 4,
`with_state` no lugar de `FromRef`, `cfg(test)` ao mover, dependências que faltavam).

**Fonte dos requisitos.**

- Pedido do Jefferson, 06/11/2026: etapas (a) criar o `hangar-core` e descer o que não depende de
  ninguém; (b) desfazer o `AppState` e o ciclo `runtime` ↔ `accounts` com trait no core; (c) tirar
  um crate por vez, começando pelos módulos soltos. Cada etapa é um PR que compila, passa nos
  testes e deixa o servidor funcionando.
- Decisão do Jefferson, 06/11/2026, depois da revisão: a troca do `RuntimeRegistry` por trait em
  `accounts` (parte da etapa b) fica para a segunda fase; a janela por Task coordena as sessões.
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
| No ciclo | 108 módulos, 247.669 linhas | 111 módulos + corpo do `lib.rs`, 248.681 linhas | Contei `crate::{…}`, `super::` e o `lib.rs` (que usa `routes::AppState` em `serve_until` e 24 módulos usam dele `AbortOnDrop`, `nodelay` ou `http_client`). Na `main` antiga o mesmo método dá 110: a diferença é o método |
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
- **Os primeiros crates de recurso, pelo que falta cortar** (módulos de topo fora do core, contados
  pela simulação): `costs` 1 (`transcript`: `py`, `pyjson`, `decode_line`, que descem na Task 2);
  `transcript` 2 módulos/3 referências; `voice` 3/4 no grafo de módulos (`groups`, `accounts`,
  `share`), mais campos do `AppState` (Task 7); `omp_sync` 4/7; `mods` 5/12; `remote_browser` 4/12;
  `speech` 6/21; `orchestration` 7/22. A contagem é por módulo de topo: cada um ainda arrasta os
  campos do `AppState` que lê, conferidos na Task.

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

Vale para toda Task que move código para o core ou para outro crate.

1. **O caminho antigo continua valendo.** O `lib.rs` do `hangar-server` troca `pub mod x;` por
   `pub use hangar_<crate>::x;` (e `pub(crate) use` para os itens de topo, como `nodelay`). Assim
   `crate::x::…` segue compilando no servidor, nos testes `tests/it` (`hangar_server::x::…`) e nas
   branches das sessões que trabalham em paralelo. Função que sai de dentro de um módulo grande
   deixa `pub use` no lugar antigo. Os re-exports só saem num PR próprio, na segunda fase.
2. **Movimento sem edição.** `git mv` dos arquivos; no mesmo PR, só o necessário para compilar:
   manifesto, re-export, `crate::` interno que passa a apontar para o crate novo, e visibilidade.
   Corte de ligação para cima (Fix A/Fix B) vai num commit anterior, que compila ainda dentro do
   servidor.
3. **Visibilidade.** Item `pub(crate)` ou privado usado pelo servidor vira `pub`; revisar cada um,
   pois vira API do crate. Inclui campo de tupla (`AbortOnDrop.0`, usado em `lib.rs:286`) e função
   citada por atributo (`deserialize_with = "crate::json_envelope::float"`, `runtime/process.rs:32`).
   Item que só o próprio crate usa continua como está. Nada de `pub` em massa.
4. **Impl com o tipo.** Impl inerente e impl de trait estrangeira (axum, serde, std) vão junto com o
   tipo, mesmo em outro arquivo (senão E0116/E0117). Impl de trait do servidor para tipo movido pode
   ficar no servidor. Nunca `impl FromRef<Arc<AppState>> for TipoDeOutroCrate`: `Arc` não é local e
   dá E0117 (ver Task 5).
5. **`cfg(test)` não atravessa crate.** `cfg!(test)` e `#[cfg(test)]` dentro do crate novo valem
   `false` quando o teste é do servidor. Antes de mover, varrer os dois no código que sai: o que
   desliga efeito real em teste (ex.: `diag.rs:40` desliga o diário com `cfg!(test)`) vira escolha
   explícita do construtor (`DiagClient` sem diário), chamada pelos testes do servidor; ajudante
   `#[cfg(test)]` usado por testes do servidor (ex.: `runtime::queue::atomic_write_inspected`,
   `queue.rs:1321`, usado por `omp_sync/mechanism_tests.rs:251` e `provider_models/tests_extra.rs`)
   fica atrás de uma feature `test-support` do crate novo, ligada só em `[dev-dependencies]` do
   servidor. Teste inline que usa só o código movido vai junto, e o arquivo de `tests/it` do servidor
   que exercita só o módulo movido também (para o `tests/it/` do crate novo): se ficar no servidor,
   mexer no crate ainda dispara o `it` grande. Teste que usa o servidor vai para
   `crates/hangar-server/tests/it/` ou para um módulo `#[cfg(test)]` do servidor. Crate novo tem os
   testes em `src/` ou num `tests/it/` único, nunca um executável por arquivo de `tests/`: cada
   executável linka o crate e as dependências dele inteiros (no servidor eram 94, de 180 a 430 MB,
   até o PR #144). Só fica à parte, num `[[test]]` próprio, o teste que mexe em estado do processo
   inteiro, como no #144.
6. **Manifesto.** `version.workspace = true`, `edition.workspace = true`, `publish = false`;
   entrada em `members` e em `[workspace.dependencies]` de `crates/Cargo.toml`; dependências pelo
   workspace, com as mesmas features que o `crates/hangar-server/Cargo.toml` declara para cada uma
   que o código movido usa: `serde`, `serde_json` e `tokio` (o comentário de lá explica a cópia
   duplicada no `target`), `reqwest` com `rustls` (`http_client`, `lib.rs:119`) e `windows-sys`
   (`install_root.rs:146`, `terminal_process.rs:262`). `#![forbid(unsafe_code)]` só em crate sem
   `unsafe`: o core não pode (`libc` em `runtime/local_policy.rs:466` e `runtime/process.rs:398`).
   Dependência externa que só o código movido usava sai do manifesto do servidor. `Cargo.lock` no
   mesmo commit.
7. **Fica no servidor:** `INTERNAL_PROTOCOL` no `lib.rs` (o `server-jobs.yml` o lê de lá), o
   `AppState`, a montagem das rotas e o `main.rs`.
8. **Portões.** CI (`server-jobs.yml:96,99`) roda `cargo test --locked --workspace`. No
   `scripts/verificar-local`, o passo completo também; o rápido (`passo_rust_rapido`) testa só o
   crate mudado quando ele é o `hangar-server` e passa a `--workspace` quando muda outro crate, o
   que já cobre os crates novos. Cobre, mas não alivia: com essa regra, mexer num crate novo
   roda o workspace inteiro, com os dois `rustc` do servidor e o `it` grande, mais do que hoje. O
   alívio depende de o passo rápido rodar os testes do crate mexido e só o `check` de quem depende
   dele, com os testes dos dependentes no CI. Essa mudança é da `verificar-local` e a política está
   em aberto com o Jefferson; este plano só registra a dependência. Código só de Windows
   (`#[cfg(windows)]`, `scope_prefix` só no Unix) só falha na matriz do CI ou na VM: a prova final
   da Task inclui o passo Windows.
9. **Na prática (aprendido nas Tasks 1 e 2).**
   - O que o servidor usa de um item que desce: procurar `modulo::nome` e `modulo::{…nome…}` em
     `src/`, `tests/` e `examples/` do servidor (o CI compila o exemplo `orq_contract`) e confirmar
     com `cargo-pinned check -p hangar-server --all-targets`. Só esses itens viram `pub`; os outros
     ficam `pub(crate)`. Aviso de "never used" no core depois da mudança aponta código morto: apagar.
   - Import com `*` não aparece na busca por nome: procurar `modulo::\*` antes de estreitar.
   - Re-export sem uso no servidor leva `#[allow(unused_imports)]` e um comentário curto dizendo
     que é de compatibilidade; o re-export não sai só porque o compilador avisou.
   - O `cargo-pinned` acrescenta `-j` no fim, então filtro de teste depois de `--` quebra. Para
     rodar só alguns testes, compilar com `--no-run` e chamar o executável de `target/debug/deps`
     direto com os filtros, gravando a saída num arquivo.
   - Teste que falha só com a suíte inteira e passa 20 de 20 sozinho é tempo sob carga: registrar
     o nome no relato e seguir; não é causado pela mudança de lugar.
   - Uso real: binário de release da branch no servidor do notebook pelo drop-in
     `CP_RUST_SERVER_BIN`, sempre com o par `hangar-server.auth.json` + `auth/` ao lado e com volta
     automática por timer do systemd; comparar as mesmas chamadas HTTP no pacote e na branch
     (`~/.hangar/teste-split/`). Cada troca derruba o servidor por alguns segundos: avisar antes.

Provas comuns, rodadas quando o Jefferson pedir, nesta ordem e uma por vez (`cargo-pinned` segura a
trava da máquina):

```bash
scripts/cargo-pinned check -p hangar-server --all-targets          # rápido: tipos, testes e exemplos
scripts/cargo-pinned test -p <crate-novo>                          # testes do crate novo
scripts/cargo-pinned test -p hangar-server --test it --no-run      # compila a integração; roda o executável direto
```

E a conferência da invariante acima (nenhuma aresta para `hangar-server`). O `scripts/verificar-local`
não roda no notebook (já derrubou sessões por falta de memória): o PR sobe com
`HANGAR_SEM_VERIFICACAO=1` e o CI do GitHub é a bateria completa, Windows incluído.

## Coordenação com as sessões que migram Python → `hangar-server`

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

Estimativa, não medida, e só para o perfil de desenvolvimento. Premissa: o pico do `rustc` cresce
com as linhas do crate. Base: 9,3 GB para ~251 mil linhas, ≈ 37 MB por mil linhas, mais ~0,5 GB
fixos de metadados das dependências (axum, tokio, reqwest, rmcp). A premissa erra para cima em
crate pequeno e para baixo no crate que monta o roteador do axum (monomorfização concentrada nele).
O pico de memória é o par biblioteca + biblioteca de teste do mesmo crate compilados juntos
(comentário em `server-jobs.yml:95`), por isso a última coluna.

| Momento | Maior crate | Pico estimado por `rustc` | Biblioteca + teste em paralelo |
|---|---|---|---|
| Hoje (medido) | `hangar-server`, ~251 mil | 9,3 GB | ~18 GB |
| Depois desta fase (Tasks 1–9; saem ~35 mil linhas) | `hangar-server`, ~216 mil | ~8,5 GB | ~17 GB |
| Crates de recurso desta fase | `hangar-voice`, ~12,7 mil | ~1 GB | ~2 GB |
| Meta da segunda fase (servidor só como topo, < 60 mil) | — | ~2,7 GB | ~5,5 GB |

O release publicado (perfil `dist`, `lto = "fat"`, `codegen-units = 1`, `crates/Cargo.toml`)
otimiza o programa inteiro na ligação: o pico dele não cai com a divisão.

**Medido na Task 1 (06/11/2026, perfil dev, commit `1ed33e29b`).** Pico de RAM + swap de cada
`rustc`, amostrado a cada 2 s em `/proc/<pid>/status` (`VmRSS` + `VmSwap`): biblioteca do
`hangar-server` 8,54 GB; executável `it` dos testes 4,66 GB; maior dependência externa (`rmcp`)
0,69 GB. O `hangar-core` compila em ~7 s e não chegou a aparecer na amostragem. Os 8,54 GB
confirmam a base da estimativa (9,3 GB medidos antes por outra sessão, com outra carga).

Leitura honesta: esta fase prova o caminho e para o crescimento do servidor (regra no
`CLAUDE.md`), mas reduz pouco o pico. O ganho real vem de desfazer o nó de 84 módulos na segunda
fase.

## Testes por módulo: o que sai junto com o crate

Contagem em `f3bc1a639` (origin/main de 11/11/2026), por leitura do texto, sem compilar nem rodar.
As linhas por módulo desta seção são desse commit; as das outras seções são de `0452a7f55`.
Pedido do Jefferson: a `verificar-local` roda os ~6.300 testes do crate a cada linha mexida, e a
divisão só alivia isso se o crate que sai levar os testes dele. Critério revisto com a sessão que
mede a `verificar-local`.

- **Tipo 1 (sai junto):** o teste exercita só a lógica do módulo. Conta o `#[test]`/`#[tokio::test]`
  de `src/` e o de `tests/it` que não sobe nada.
- **Tipo 2b (sai junto, é lento):** sobe processo externo (tmux, psmux, `ProcessMux`, o cano), sem
  precisar do servidor linkado.
- **Tipo 2a (fica no `it` do servidor):** o teste, ou um ajudante que ele chama no mesmo arquivo ou
  nas pastas de apoio do `it`, usa `AppState`, `spawn_server`/`spawn_state`/`serve_mods`,
  `hangar_server::routes::`, `RuntimeRegistry`, `ListHub` ou `CARGO_BIN_EXE` (que só existe para
  binário do próprio pacote, aqui o do servidor). Teste que cai nos dois critérios conta como 2a.
- **Módulo de um arquivo do `it`:** o prefixo do nome do arquivo (`share_gate.rs` → `share`); sem
  prefixo que case, o módulo de `hangar_server::` mais citado nele.
- **Usa do servidor:** quantos outros módulos de topo com arquivo em `src/` o módulo cita por
  `crate::`, teste incluído. O que o `lib.rs` reexporta do core (`auth`, `http_cors`…) fica fora;
  `diag` e `transcript` ainda têm arquivo em `src/` e contam, então o número é teto do que falta
  cortar.
- **Toques em 30 dias:** arquivos do módulo tocados por commit na origin/main
  (`git log --since="30 days ago" --no-merges --name-only --format= origin/main --
  crates/hangar-server/src`, agrupado pelo 4º componente do caminho). É o quanto mexer nele dispara
  a verificação.

| Módulo | Linhas | Tipo 1 (src + it) | Tipo 2b (src + it) | Tipo 2a (src + it) | Sai junto | Usa do servidor | Toques em 30 dias |
|---|---:|---:|---:|---:|---:|---:|---:|
| `runtime` | 33.920 | 866 (225 + 641) | 2 (2 + 0) | 180 (39 + 141) | 83% | 25 | 708 |
| `voice` | 13.169 | 314 (314 + 0) | 0 (0 + 0) | 13 (0 + 13) | 96% | 8 | 111 |
| `accounts` | 18.019 | 270 (218 + 52) | 8 (8 + 0) | 105 (3 + 102) | 73% | 16 | 234 |
| `list` | 12.563 | 180 (142 + 38) | 21 (21 + 0) | 49 (3 + 46) | 80% | 34 | 212 |
| `session_open` | 12.734 | 180 (180 + 0) | 15 (15 + 0) | 12 (1 + 11) | 94% | 29 | 532 |
| `share` | 3.271 | 184 (2 + 182) | 0 (0 + 0) | 148 (0 + 148) | 55% | 12 | 56 |
| `mods` | 4.000 | 165 (4 + 161) | 9 (0 + 9) | 54 (0 + 54) | 76% | 7 | 16 |
| `costs` | 5.593 | 166 (10 + 156) | 0 (0 + 0) | 0 (0 + 0) | 100% | 3 | 13 |
| `update` | 11.737 | 149 (119 + 30) | 0 (0 + 0) | 23 (0 + 23) | 87% | 19 | 174 |
| `groups` | 6.943 | 140 (23 + 117) | 0 (0 + 0) | 131 (0 + 131) | 52% | 15 | 179 |
| `session_write` | 6.362 | 122 (73 + 49) | 0 (0 + 0) | 88 (2 + 86) | 58% | 28 | 234 |
| `state` | 6.506 | 109 (104 + 5) | 6 (5 + 1) | 8 (8 + 0) | 93% | 16 | 78 |
| `speech` | 7.650 | 114 (113 + 1) | 0 (0 + 0) | 10 (2 + 8) | 92% | 11 | 96 |
| `terminal_input` | 1.870 | 103 (4 + 99) | 5 (0 + 5) | 0 (0 + 0) | 100% | 3 | 26 |
| `search` | 4.411 | 98 (98 + 0) | 0 (0 + 0) | 21 (1 + 20) | 82% | 17 | 124 |
| `remote_browser` | 2.064 | 66 (5 + 61) | 0 (0 + 0) | 8 (0 + 8) | 89% | 8 | 51 |
| `connect` | 611 | 64 (0 + 64) | 0 (0 + 0) | 76 (0 + 76) | 46% | 7 | 42 |
| `plugin` | 2.176 | 54 (43 + 11) | 2 (1 + 1) | 9 (0 + 9) | 86% | 8 | 50 |
| `pages` | 1.759 | 50 (50 + 0) | 0 (0 + 0) | 11 (0 + 11) | 82% | 4 | 25 |
| `codex_setup` | 7.306 | 46 (33 + 13) | 0 (0 + 0) | 2 (2 + 0) | 96% | 17 | 235 |
| `orchestration` | 5.768 | 45 (34 + 11) | 0 (0 + 0) | 5 (4 + 1) | 90% | 10 | 123 |
| `uploads` | 2.507 | 45 (11 + 34) | 0 (0 + 0) | 7 (3 + 4) | 87% | 8 | 17 |
| `vault` | 4.799 | 45 (45 + 0) | 0 (0 + 0) | 22 (22 + 0) | 67% | 8 | 108 |
| `loops` | 2.416 | 44 (44 + 0) | 0 (0 + 0) | 6 (2 + 4) | 88% | 19 | 44 |
| `side` | 2.718 | 44 (44 + 0) | 0 (0 + 0) | 4 (4 + 0) | 92% | 18 | 39 |
| `provider_models` | 3.891 | 30 (30 + 0) | 0 (0 + 0) | 12 (12 + 0) | 71% | 11 | 48 |
| `device_install` | 1.842 | 26 (26 + 0) | 0 (0 + 0) | 15 (0 + 15) | 63% | 4 | 32 |
| `push` | 2.942 | 26 (26 + 0) | 0 (0 + 0) | 5 (5 + 0) | 84% | 17 | 65 |
| `diag_report` | 2.050 | 24 (24 + 0) | 0 (0 + 0) | 11 (5 + 6) | 69% | 9 | 40 |
| `dictation` | 4.218 | 24 (24 + 0) | 0 (0 + 0) | 41 (0 + 41) | 37% | 13 | 50 |
| `subagents` | 1.825 | 22 (22 + 0) | 0 (0 + 0) | 0 (0 + 0) | 100% | 2 | 18 |
| `machine_peers` | 3.629 | 19 (3 + 16) | 0 (0 + 0) | 6 (0 + 6) | 76% | 14 | 80 |
| `transcription` | 1.542 | 17 (17 + 0) | 0 (0 + 0) | 5 (5 + 0) | 77% | 5 | 31 |
| `config_sync` | 5.543 | 16 (16 + 0) | 0 (0 + 0) | 1 (1 + 0) | 94% | 21 | 104 |
| `mcp` | 1.604 | 16 (16 + 0) | 0 (0 + 0) | 21 (0 + 21) | 43% | 12 | 63 |
| `session_media` | 555 | 11 (4 + 7) | 0 (0 + 0) | 31 (0 + 31) | 26% | 6 | 8 |
| `commands` | 1.628 | 9 (1 + 8) | 0 (0 + 0) | 6 (0 + 6) | 60% | 6 | 7 |
| `harness_setup` | 3.514 | 9 (1 + 8) | 0 (0 + 0) | 1 (0 + 1) | 90% | 19 | 60 |
| `term` | 1.629 | 6 (6 + 0) | 2 (2 + 0) | 15 (15 + 0) | 35% | 6 | 9 |
| `archive_list` | 814 | 6 (6 + 0) | 0 (0 + 0) | 36 (0 + 36) | 14% | 13 | 12 |
| `worktree_routes` | 715 | 6 (2 + 4) | 0 (0 + 0) | 42 (1 + 41) | 12% | 8 | 9 |
| `login_guests` | 2.417 | 4 (1 + 3) | 0 (0 + 0) | 3 (0 + 3) | 57% | 14 | 82 |
| `engines` | 1.951 | 2 (0 + 2) | 0 (0 + 0) | 6 (0 + 6) | 25% | 13 | 31 |
| `routes` | 1.977 | 1 (1 + 0) | 0 (0 + 0) | 21 (12 + 9) | 5% | 92 | 175 |
| `workspace_routes` | 852 | 1 (1 + 0) | 0 (0 + 0) | 49 (0 + 49) | 2% | 11 | 17 |
| `costs_routes` | 443 | 0 (0 + 0) | 0 (0 + 0) | 50 (0 + 50) | 0% | 2 | 3 |
| outros 72 (módulos menores, `lib.rs`, `main.rs`; nos toques, também os que já saíram, como `omp_sync`) | 26.863 | 430 (317 + 113) | 17 (0 + 17) | 243 (20 + 223) | 65% | — | 927 |
| executáveis à parte (22 arquivos) | — | 207 | 1 | 44 | 83% | — | — |
| **Total** | 253.316 | 4.575 | 88 | 1.656 | 74% | | 5.398 |

Total: 6.319 testes. Saem junto 4.663 (74%): 4.575 do tipo 1 e 88 do 2b. Ficam 1.656 do 2a. Por
lugar: `src/` tem 2.708 (2.482 + 54 + 172), o `tests/it` tem 3.359 (1.886 + 33 + 1.440) e cerca de
20 executáveis à parte (22 arquivos em `tests/*.rs` e `tests/costs_hook`) têm 252 (207 + 1 + 44).

O que a tabela mostra:

- **O alívio do dia a dia só chega com o nó** `runtime`/`accounts`/`list`/`session_open`/
  `session_write`/`state`: ele tem 1.998 dos 5.398 toques de 30 dias (37%) e 1.779 dos 4.663 testes
  que saem (38%); `runtime` sozinho tem 708 toques e 868 testes. É o corte mais caro. Os arquivos
  grandes do `runtime` no `it` (`terminal_runtime.rs`, `runtime_claude.rs`) citam `terminal_facts`,
  `term`, `plugin`, `interactions`, `terminal_input` e `mods`: saem quando esses módulos estiverem
  fora.
- **1.919 testes que saem moram no `tests/it` do servidor.** Eles não saem sozinhos: mover o módulo
  sem levar esses arquivos para o `tests/it` do crate novo deixa a verificação igual. Só os de
  `src/` saem de graça.
- **Com tudo extraído, ficam no servidor 1.656 testes**: 1.440 no `it`, 172 em `src/` e 44 em
  executáveis à parte. Não somem: são os que precisam do servidor inteiro.
- **Rende pouco na verificação** (quase tudo 2a, mexer neles ainda dispara o `it` grande):
  `workspace_routes` 2%, `routes` 5%, `worktree_routes` 12%, `archive_list` 14%, `session_media`
  26%, `term` 35%, `dictation` 37%, `mcp` 43%, `connect` 46%. `costs_routes` está em 0%: os 50 testes dele montam o
  `AppState` e o roteador do servidor (`tests/it/costs_routes.rs`). Só saem se o apoio do teste for
  reescrito sobre o `CostsState` da Task 5; mover o arquivo não basta.
- **140 testes de `connect` e 36 de `transcript` ainda estão no `it` do servidor.** `transcript` já
  é crate; `connect` tem a supervisão no `hangar-connect` e o porteiro e as rotas no servidor (611
  linhas). Dos tipo 1 (64 e 16), o que exercita só o que já está no crate pode ir para ele,
  conferindo teste a teste: os arquivos `connect_*` misturam os dois.
- **Cerca de 20 executáveis à parte ainda linkam o servidor inteiro** (252 testes). `hangar-package`
  e `hangar-transcript` têm 2 arquivos em `tests/` cada, contra a regra do item 5 do contrato.
- **Número de testes não é tempo.** Medido em 10/11 pela sessão da `verificar-local`, com 4 threads:
  o `it` leva 694 s e a lib 138 s. O libtest estável não dá tempo por teste, então não está
  confirmado que o tempo do `it` é o do tipo 2a. Os lentos que aparecem nos logs:
  `plugin_loopback::held_routes`, `runtime_queue::state_stays_bounded_with_100kb_replies` e
  `worktrees::tests`.
- **A seleção por crate vai render:** 985 de 1.589 commits tocam um módulo só (medido pela mesma
  sessão; mediana 1, p90 4).

## Ordem e paralelismo

As Tasks são uma cadeia: cada uma usa o que a anterior desceu e move arquivos que a próxima toca.
Nenhuma roda em paralelo com outra. Tasks de base ou risco, com revisão: 1 (contrato de manifesto
que todas seguem), 3 (efeitos que só os testes desligavam), 4 (autorização do dono) e 5 (padrão de
estado que todas seguem). A inversão `accounts` → `RuntimeRegistry` ficou para a segunda fase
(seção "Adiado").

Os módulos soltos do pedido: dos 11 medidos, 9 entram no core na Task 1. `plugin_listener` (114
linhas, usado só por `routes`) e `orq_storage` (627, usado só por `main.rs`) não ganham crate
próprio: o ganho de memória é nulo e crate com uma responsabilidade tão pequena não se paga
(`rust-crate-architecture`, "Crate or module?"). `orq_storage` vai com `orchestration` na segunda
fase; `plugin_listener` fica no servidor. Depois do core, a ordem segue o que falta cortar (seção
"Números conferidos").

**Ordem revista pelos testes e pela frequência de mudança (11/11/2026).** Seção "Testes por
módulo". O alívio na verificação só chega com o nó: os primeiros cortes da segunda fase quase não
mudam o dia a dia e valem como pré-requisito dele.

1. Nesta fase a ordem fica: Task 5 e 6 (`costs`: 166 testes, mais os 71 de `tests/costs_hook`; 13
   toques em 30 dias; os 50 de `costs_routes` só com o apoio reescrito) e Task 7 (`voice`: 314,
   todos em `src/`; 111 toques).
2. Segunda fase, primeiro o que destrava o nó, nesta ordem: `interactions` (14 testes),
   `terminal_input` (108 em 1.870 linhas), `term` (8) e `terminal_facts` (794 linhas), `mods` (174)
   e `plugin` (56). Todos citam módulos do nó (`runtime`, `session_write`, `state`, `list`) e o
   `runtime` os usa de volta (16 arquivos dele citam `terminal_input`, 14 `interactions`, 13
   `mods`, 12 `terminal_facts`): cada um começa por um commit de corte (Fix A ou Fix B), senão o
   crate novo dependeria do servidor. O tamanho desses cortes não está medido; é a primeira conta
   da Task 10.
3. Avaliar cedo, na Task 10: `codex_setup` (235 toques, 96% sai, 7.306 linhas) e `update` (174
   toques, 87% sai, 11.737 linhas). Mudam muito e quase tudo sai; o custo é o que usam do servidor
   (17 e 19 módulos).
4. O nó (`runtime` 868, `accounts` 278, `list` 201, `session_open` 195, `session_write` 122,
   `state` 115): depende da inversão da seção "Adiado" e do item 2.
5. Depois, porque não destravam o nó e mudam pouco: `speech` (114), `remote_browser` (66),
   `orchestration` e `uploads` (45 cada) e os pequenos `pages` (50), `permission_control` (35),
   `subagents` (22) e `tail` (20). `share` (184 de 332) e `groups` (140 de 271) deixam metade no
   `it` de qualquer jeito.
6. Sem pressa, porque não aliviam a verificação: os módulos de rota listados na seção de testes
   (menos `term`, que entra no item 2 por destravar o nó).

O core é um crate só, com módulos por assunto (`net`, `http`, `pycompat`, `fs`, `process`, `paths`,
`edge`…). Separar a parte pura num crate próprio não reduz memória; fica para quando houver motivo
(outro consumidor, regra de `unsafe` diferente).

### Task 1: Criar o `hangar-core` com os módulos que não dependem de ninguém

Status: ready-for-agent
Revisão: base — define o manifesto e o re-export que todas as Tasks repetem.

Arquivos: `crates/hangar-core/{Cargo.toml,src/lib.rs}` (novos); `git mv` de
`crates/hangar-server/src/{warn_limit,http_contract,http_route_response,http_body_limit,json_envelope,value_coercion,terminal_state,terminal_lock,costs_failure}.rs`
para `crates/hangar-core/src/`; `crates/Cargo.toml`; `crates/hangar-server/{Cargo.toml,src/lib.rs}`;
`crates/Cargo.lock`.

Do corpo do `lib.rs` descem `http_client()`, `nodelay()` e `AbortOnDrop` (módulo `net` do core,
campo `.0` público), usados por 24 módulos como `crate::http_client`, `crate::nodelay` e
`crate::AbortOnDrop`; o `lib.rs` os re-exporta com a mesma visibilidade. `write_test_executable`
fica: nenhum dos nove módulos o usa, e ele desce com quem o usar.

- [x] **Step 1: Conferir que os nove módulos e os três itens do `lib.rs` não usam nada do servidor**
  `grep -nE 'crate::|super::|cfg!\(test\)|cfg\(test\)' <arquivos>` → só `http_route_response` →
  `http_contract` (que desce junto) e os testes inline deles. Qualquer outra referência: parar e
  trazer para o Jefferson.
- [x] **Step 2: Criar o crate e mover os arquivos** conforme os itens 2 e 6 do contrato
  (`json_envelope::float` vira `pub`, item 3).
- [x] **Step 3: Ajustar visibilidade e re-exports** (itens 1 e 3 do contrato).
- [x] **Step 4: Provas comuns** (quando autorizadas) e a invariante sem aresta para o servidor.
  Feito: `build -p hangar-server` ok; `test -p hangar-core` 6/6; `test -p hangar-server --test it`
  3258 ok, 1 falha de tempo sob carga (`session_write_routes::gate_held_by_a_transfer_answers_409_without_waiting`,
  exige < 1 s; sozinho passou 20/20; não toca no que foi movido). `verificar-local` fica para o PR.
- [x] **Step 5: Medir a memória** de um `rustc` do `hangar-core` e do `hangar-server` (quando
  autorizado; RAM + swap do processo) e anotar na seção de estimativa.
- [x] **Step 6: Uso real — verificação manual** `systemctl --user restart hangar-backend.service`
  com o binário novo; abrir o app, ver a lista de sessões e mandar uma mensagem numa sessão.
  Feito junto com a Task 2 (06/11/2026, 21:22–21:31): release da branch no ar por um drop-in
  `CP_RUST_SERVER_BIN` (scripts em `~/.hangar/teste-split/`, com volta automática por timer do
  systemd); saúde com os quatro donos e a porta de convidado; o Jefferson mandou mensagem numa
  sessão pelo celular ("parece normal"). Depois, as mesmas chamadas HTTP no binário do pacote e no
  da branch (`~/.hangar/teste-split/sondar.sh`, 21:33–21:34), todas iguais: `/api/costs` (200,
  gzip, CORS, mesmas chaves, 5.342 sessões, 97 dias, 22 modelos; o custo andou US$ 1,12 entre as
  leituras, uso das sessões abertas), condutor da orquestração `2026-11-04-pr309` (corpo idêntico),
  sessão inexistente (404 com o mesmo corpo e CORS), `/api/cotas?forcar=talvez` (422
  `bool_parsing` idêntico), porta de convidado e porta do dono sem token (401 idêntico). Logo
  depois de subir, Custos responde 202 "aquecendo" até o índice ficar pronto, como no pacote. O binário precisa ir com o par `hangar-server.auth.json` + `auth/`, senão o
  Python recusa ("sem par válido da biblioteca de autenticação").

### Task 2: Descer `auth`, `config` e os ajudantes de resposta HTTP e JSON (Fix A)

Status: ready-for-agent (depois da Task 1)

Corta as ligações para cima dos módulos-base. Cada função listada desce inteira, com os testes
dela, para o módulo do core indicado; o lugar antigo fica com `pub use`.

| O quê | De | Para (`hangar-core`) |
|---|---|---|
| `cors`, `maybe_gzip`, `gzip_with_minimum`, `not_found` (funções puras sobre cabeçalhos) | `routes.rs` | `http` |
| `json_response`, `detail`, `detail_body`, `too_large`, `BODY_LIMIT`, `busy_body` (+ `BUSY_MSG`) | `session_write/mod.rs` | `http::response` |
| `py`, `pyjson`, `json_tree`, `json_writer`, `decode_line` (`transcript/mod.rs:65`) | `transcript/` | `pycompat` |
| `load`, `py_str` (e o que só elas usam) | `orq_conductor/mod.rs` | `pycompat` |
| `codec`, `truthy` (definido em `common.rs:126`, exposto como `orchestration::truthy` por `pub use common::*`), `body::parse` com `body_encoding` e `body_numbers` | `orchestration/` | `pycompat` / `http::body` |
| módulos inteiros `auth`, `config`, `query`, `http_cors`, `http_body_input` | topo | mesmos nomes |

Executado no commit `4ef8e2fcd`, com três ajustes à tabela:

- Arquivos inteiros, sem recortar funções: `orq_conductor/mod.rs` (752 linhas) só dependia de
  `transcript::{py, pyjson}` e desceu todo como `hangar_core::orq_codec::conductor`; a rota
  (`orq_conductor/http.rs`) fica no servidor, num `orq_conductor/mod.rs` que re-exporta o feed.
  `codec`, `body`, `body_encoding` e `body_numbers` desceram inteiros para `orq_codec/`;
  `py`, `pyjson`, `json_tree` e `json_writer` para `pycompat/`, com `decode_line` e `truthy`.
- Nos arquivos de compatibilidade com o Python, todo `pub(crate)` virou `pub`: eles serviam o
  servidor inteiro. `body_encoding` ficou público porque `update/git_engine/system.rs:342` o usa.
- `auth.rs` e `config.rs` ficam no servidor até a branch `connect-rust` entrar na `main`: ela
  acrescenta neles código que usa `crate::devices` e `crate::connect` (janela por Task).

- [x] **Step 1: Listar as dependências de cada item** e confirmar que, depois da tabela inteira,
  nenhum aponta para o servidor (`codec` usa `orq_conductor::{load,py_str}` e `transcript::pyjson`,
  que descem juntos; `auth` usa `session_write::json_response`; `config` usa `auth`; `too_large`
  usa `cors`). Varrer `cfg!(test)`/`#[cfg(test)]` (item 5 do contrato).
- [x] **Step 2: Commit de corte** com as funções movidas para o core e `pub use` nos lugares antigos.
- [x] **Step 3: Commit de movimento** dos módulos inteiros (sem `auth` e `config`), com re-export no `lib.rs`.
- [x] **Step 4: Provas comuns** (quando autorizadas). Feito: `check` do core e do servidor sem
  aviso nos arquivos tocados; `test -p hangar-core` 7/7; testes internos do servidor 2801 ok e 1
  falha de tempo sob carga (`session_open::tests::the_terminal_command_stops_a_stuck_reconciliation_at_the_limit`,
  sozinho 20/20); `it` 3259/3259.
- [x] **Step 6: Mover `auth` e `config`** depois do merge da `connect-rust`, com as mesmas provas.
  Feito (08/11/2026): `auth.rs` e `config.rs` no core. O Connect trouxe duas ligações novas:
  `CONNECT_PEER` (em `devices.rs`) desceu para o `auth`, com `pub use` em `devices.rs`;
  `connect_listen`/`preview_listen` usam as portas do `hangar-connect`, que depende do core, então
  ficam no `config.rs` do servidor, que re-exporta `hangar_core::config::*`. O core passou a
  depender de `hangar-auth`, `subtle` e `form_urlencoded`. Provas: `check` do core e do servidor
  (`--all-targets`) limpos; core 29 ok; servidor 2608 ok; `it` 3392 ok.
- [x] **Step 5: Uso real — verificação manual** login do dono e de convidado (porta 8766) e uma
  chamada que responde erro JSON (ex.: sessão inexistente devolve o `detail` com `code`).
  Feito junto com a Task 1 (ver o Step 6 dela): uso do dono pelo celular; convidado não exercitado.

### Task 3: Descer os ajudantes de arquivo, processo e caminho (Fix A)

Status: ready-for-agent (depois da Task 2)
Revisão: risco — efeitos que hoje só os testes desligam (diário, escrita de arquivo).

Vem antes do portão (Task 4), porque o portão precisa do `share::store`, que usa `atomic_write` e
`random_hex` (`share/store.rs:127,194`), e do `diag` (`route_failed`, `routes.rs:303`).

**Executado (07/11/2026) com escopo reduzido.** Desceram: `fs` (`QueueRefusal`, `invalid`,
`read_replaced*`, `replace`, `atomic_write`, `atomic_write_inspected` atrás de `test-support`),
`queue_row::row_waits`, `ids::random_hex`, `process::vanished`, `net::{HttpClient, POOL_IDLE,
client}`, `account_env` (o antigo `accounts/environment.rs` inteiro, com `Account` e os caminhos do
omp em `account_env/omp.rs`, mais `valid_omp_profile`), e os módulos inteiros `terminal_process` e
`display_env` (o log dele passou a `tracing::info!(target: "voice", …)`, igual ao `voice::log`).
O core perdeu o `#![forbid(unsafe_code)]`: `terminal_process` tem `unsafe` de `libc` e `windows-sys`.
Ficaram para depois:
- `diag`, `install_root` e `user_data` (este depende do `install_root`), com `log_base*`,
  `pointer`, `is_backend` e `without_verbatim`, que só eles usam: a sessão `migrar-rust-2` tem
  commit local mexendo em `diag.rs` e `install_root.rs` (janela por Task). Movem depois do merge dela.
- `terminate`, `scope_prefix` e `resolve_program`: só cortariam `accounts` → `runtime`, cuja inversão
  foi adiada para a fase 2; `scope_prefix` arrasta as constantes e a sondagem do systemd.
  `resolve_program` desce na Task 9, que precisa dela.

| O quê | De | Para (`hangar-core`) |
|---|---|---|
| `atomic_write`, `replace`, `read_replaced`, `read_replaced_to_string`, `row_waits` (`queue.rs:566`); `atomic_write_inspected` atrás de `test-support` | `runtime/queue.rs` | `fs` |
| `terminate` (com `ProcessError`), `scope_prefix` (só Unix), `resolve_program` | `runtime/process.rs` | `process` |
| `log_base`, `log_base_in` | `runtime/local_policy.rs` | `paths` |
| `random_hex` | `mods/state.rs:293` | `net` |
| `AccountEnvironment` (`accounts/environment.rs`), com o que ele usa: `catalog::Account`, `secondary_auth::omp_directories` → `list::discover_other::valid_omp_profile` | `accounts/`, `list/` | `accounts_env` |
| `pointer`, `is_backend` | `session_open/cli.rs` | `paths` |
| `without_verbatim`, `vanished` | `list/links.rs`, `list/procs.rs` | `fs`, `process` |
| módulos inteiros `terminal_process`, `display_env`, `diag`, `user_data`, `install_root` | topo | mesmos nomes |

- `display_env` serve a voz, `accounts/login_browser.rs:846`, `accounts/login_view.rs:175` e
  `session_open/terminal.rs:62,638`: vai para o core, e as duas chamadas a `crate::voice::log` viram
  o mesmo `tracing::info!(target: "voice", …)` que `voice::log` faz (`voice/mod.rs:23`).
- `diag.rs:40` (`if cfg!(test) { None }`): o `DiagClient` ganha um construtor sem diário, e os
  testes do servidor passam a usá-lo (item 5 do contrato).
- O teste de `install_root` só de Windows usa `update::make_link` (`install_root.rs:221`): fica no
  servidor como teste de integração.

**Concluído (09/11/2026), depois do merge do #337.** Desceram `user_data`, `paths` (`log_base*`,
`pointer`, `is_backend`, `current`), `diag` e a parte pura do `install_root` (`root`, `python_exe`,
`backend_command`). Ficam no servidor o `version_route` do #337 (lê `AppState` e `boot_version`) no
`diag.rs` e o lançador (`launch`, `run`, Job do Windows) no `install_root.rs`, que re-exportam o core.
O `cfg!(test)` do `DiagClient` não basta fora do crate: o diário agora fecha em binário de teste do
cargo (`process::cargo_test_binary`, a checagem que o `push/send.rs` já tinha), e o `diary_dir_in`
também quando o HOME é o real. Isso fechou um vazamento antigo: o `it` gravava sessões de teste no
diário real (visto nos dias 06 e 07/11).

- [x] **Step 1: Listar as dependências de cada item**, inclusive de `catalog::Account`, e
  confirmar que a tabela fecha sem nada do servidor; trazer ao Jefferson o que puxar módulo grande.
- [x] **Step 2: Varrer `cfg!(test)`/`#[cfg(test)]`** no que desce e decidir cada caso pelo item 5.
- [x] **Step 3: Commit de corte** (funções e tipos) com `pub use` nos lugares antigos.
- [x] **Step 4: Commit de movimento** dos cinco módulos inteiros.
- [x] **Step 5: Provas comuns** (quando autorizadas), com o passo Windows (`scope_prefix`, `windows-sys`).
  Feito: `check` do core e do servidor limpos; core 41 ok; servidor 2627 ok; `it` 3604 ok e 5 falhas
  de testes novos do #337 que passam sozinhos (10/10 filtrados) ou pedem o `backend/.venv` do
  checkout (`s01`); avisado à sessão do #337. Passo Windows fica com o CI.
- [x] **Step 6: Conferir que os testes não gravam no diário real** (`~/.hangar/…/diario/servidor-rust`
  sem linha nova depois da suíte). Feito: zero linha nova com sessão que não está viva.
- [ ] **Step 7: Uso real — verificação manual** abrir sessão Claude e Codex, login de conta pelo
  navegador, e "Reportar problema" gera o diário.

### Task 4: Portão do dono no core

Status: ready-for-agent (depois da Task 3)
Revisão: risco — autorização do dono e do convidado.

**Escopo reduzido em 09/11/2026 (decisão do Jefferson).** O texto original mandava descer também o
repasse ao Python (`pass`, `proxy`, a assinatura do convidado e a troca do `Watcher` por uma
marca). A retirada da reserva Python (R2 da `migracao-python-rust-status`) troca o `!owner → pass`
por `owner_gate` e encolhe esse repasse, então ele fica no servidor e a R2 mexe nele lá. Só a
autorização desce.

Os módulos de recurso usam `routes::{gate, owner_gate, who, forward_for, on_guest_port,
route_failed}` recebendo o `AppState` inteiro. Eles leem só `cfg.trusted`, `auth`, `diag` e marcas
nas extensões do pedido. No core (`hangar_core::edge`) viram funções livres que recebem essas
peças, mais a struct que um recurso guarda no estado dele:

```rust
pub struct Edge { pub trusted: TrustedHosts, pub auth: Auth, pub diag: DiagClient }
impl Edge {
    pub fn gate(&self, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<(Forward, bool), AuthError>> + Send + use<>;
    pub fn owner_gate(&self, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<Forward, Response>> + Send + use<>;
    // who, forward_for, route_failed: idem
}
```

- Desceram só o que o portão lê: `Forward`, `RequestAuth`, `session_write::bridge::BridgedOwner`
  (`hangar_core::edge`), `share::gate::GuestConn`, `share::caller::{Caller, GuestGrant, caller}` e
  `share::store::{Guest, Kind, Share}` (`hangar_core::guest`).
- `login_guests::gate::Login` não desce: carrega o cadastro do convidado, que depende de fila,
  troca de nome e contas. O portão não precisa dele: os dois porteiros de convidado (8766 e login)
  põem um `Caller` no pedido, e é a presença dele que tira o pedido do portão do dono.
- Ficam no servidor: `routes::pass` (com o gancho de `/api/config`), `proxy.rs`, o
  `share/forward.rs` (assinatura do convidado), `Forwarded`, `AUTH_EMPTY_QUERY`, `CLOSE_ENDED`,
  `preflight`, `is_websocket` e o `share::gate::Watcher`: nada no core os usa. Recurso que ainda
  repassa (custo de sessão para convidado) recebe o repasse como função no estado dele, na Task 5.
  A voz lê o `Watcher` direto: a marca de fim de acesso é decidida na Task 7, com o consumidor na
  frente.
- O `AppState` não ganha campo `edge`: os testes trocam `state.auth` e `state.diag` depois de
  montar o estado, e uma cópia guardada ficaria velha. As funções de `routes` passam as peças do
  `AppState` às funções livres, com a mesma assinatura: nenhum chamador muda nesta Task. O `Edge`
  de cada recurso é montado junto com as rotas dele (Task 5).

- [x] **Step 1: Inventariar o que o portão lê** (`routes.rs` de `gate` a `pass`, `proxy.rs`,
  `share/{gate,forward,caller,store}.rs`, `BridgedOwner`, `Login`) e conferir que nenhum outro
  campo do `AppState` entra.
- [x] **Step 2: Descer tipos e marcas** (Fix A), com `pub use` nos lugares antigos.
- [x] **Step 3: Criar `Edge` e delegar** as funções de `routes` às do core.
- [x] **Step 4: Provas comuns** (quando autorizadas), com atenção aos testes de autorização em
  `tests/it` (dono, convidado, bloqueio por tentativas, recibo de admissão, convite revogado).
  Feito: `check --tests` do core e do servidor, `verificar-local` e o CI do PR.
- [ ] **Step 5: Uso real — verificação manual** dono pelo token, dono pelo cookie, convidado por
  convite na 8766, convidado por login, e token errado recusado com 401 e depois 429.

### Task 5: Estado por recurso no lugar do `AppState` — piloto em `costs_routes`

Status: ready-for-agent (depois da Task 4)
Revisão: base — define o padrão que cada crate de recurso repete.

O `AppState` continua sendo a raiz que o servidor monta; o que acaba é recurso depender dele.
Cada recurso declara um estado concreto (sem genérico, para a geração de código ficar no crate
dele) e entrega handlers; o servidor monta a rota com o estado aplicado, mantendo o repasse:

```rust
// no recurso
#[derive(Clone)]
pub struct CostsState { pub edge: Arc<Edge>, pub costs: Arc<Collector>, pub fx: Arc<Fx>, /* … */ }
pub async fn report(State(st): State<CostsState>, /* … */) -> Response;

// no hangar-server, onde hoje está get(costs_routes::report).fallback(pass_any)
.route("/api/costs", get(costs::report).with_state::<Arc<AppState>>(costs_state.clone()).fallback(pass_any))
```

`FromRef` não serve: `impl FromRef<Arc<AppState>> for CostsState` dá E0117 quando `CostsState`
morar em `hangar-costs` (item 4 do contrato).

O que `costs_routes` lê além do portão: `costs`, `fx`, `reports`, `diag` (`:182`), a origem das
skills (`AppState::skill_origins`, campos `origins`/`origins_home`, `routes.rs:275`), `who` com
`Caller::can` (`:309`) e `AppState::info` (`:313`), que roda a descoberta da sessão e devolve
`transcript::InternalInfo`. `info` entra no `CostsState` como trait do core (`SessionLookup`) com só
o que `costs_routes` usa do resultado; `InternalInfo` não desce.

- [ ] **Step 1: Listar os campos e métodos do `AppState` que `costs_routes` usa** e montar
  `CostsState` só com eles.
- [ ] **Step 2: Escrever o trait `SessionLookup`** com o mínimo que `costs_routes.rs:313` em diante
  consome, implementado pelo servidor.
- [ ] **Step 3: Trocar `State<Arc<AppState>>` por `State<CostsState>`** e as chamadas a `routes::*`
  por `st.edge.*`; montar as rotas com `with_state` como acima.
- [ ] **Step 4: Provas comuns** (quando autorizadas); `grep -n AppState crates/hangar-server/src/costs_routes.rs` → vazio.
- [ ] **Step 5: Uso real — verificação manual** abrir Custos no app e conferir que os números
  batem com os de antes da troca.

### Task 6: Extrair `hangar-costs`

Status: ready-for-agent (depois da Task 5)

`costs` (5.121 linhas) usa do servidor `transcript::{py, pyjson, decode_line}` (core pela Task 2),
`proxy::client` (core pela Task 4), `warn_limit` e `http_client` (core pela Task 1); `costs_routes`
já usa `CostsState` (Task 5).

- [ ] **Step 1: Conferir** `grep -rnE 'crate::' crates/hangar-server/src/{costs,costs_routes.rs}` →
  só `crate::costs`, `crate::costs_routes` e itens re-exportados do core.
- [ ] **Step 2: Mover `costs/` e `costs_routes.rs`** para `crates/hangar-costs/` pelo contrato.
  Vão junto, num `tests/it/` só, os arquivos do tipo 1 que hoje estão no `it` do servidor
  (`contract_costs_*`, `costs_*_unit`, `costs_fx`, `costs_origins`, `costs_fast_paths`,
  `scopes_native`; 156 testes), com o apoio `tests/it/common/costs.rs` que três deles e o
  `costs_hook` importam. Os testes trocam `hangar_server::costs` por `hangar_costs` (edição que o
  item 2 do contrato permite) e o crate novo ganha `hangar-transcript` em `[dev-dependencies]`
  (`contract_costs_reports.rs`). `tests/costs_hook/*` (71 testes) vai como `[[test]]` próprio do
  `hangar-costs`, porque instala o gancho de pânico do processo inteiro (exceção do item 5); antes,
  `install_panic_hook` (`lib.rs:334`) desce para o core, senão os testes que o chamam ficam no
  servidor. Os 50 de `tests/it/costs_routes.rs` vão se montarem só o
  `CostsState`; hoje todos montam o `AppState`, então ficam até o apoio ser reescrito.
- [ ] **Step 3: Provas comuns** (quando autorizadas), incluindo os testes movidos no Step 2 e os
  que ficaram no servidor citando `hangar_server::costs` (`tests/it/costs_routes.rs`).
- [ ] **Step 4: Uso real — verificação manual** Custos e Uso no app.

### Task 7: Extrair `hangar-voice`

Status: ready-for-agent (depois da Task 6)

`voice` (12.717 linhas) usa do servidor: `groups::peers` (`PeerClient`, `PeerBook`, `PeerError`,
`TOO_LARGE`), `st.groups.server_id()` (`voice/hub.rs:178`), `st.peers`, `accounts` em
`hub.rs:232-242` (`codex_command()`, `visible_codex_accounts()` que devolve `catalog::Account`, e
`env.codex(&account)`), `share::gate::preflight` (desce nesta Task, ou troca pelo `http_cors::preflight` do core), `display_env` e
`terminal_process::PRIVATE_ENV_KEYS` (core pela Task 3).

- `groups::peers` (409 linhas) usa `diag::record_blocking`, `random_hex`, `transcript::py`,
  `warn_limit`, `http_client` (todos no core) e `groups::model::PROTOCOL` (`peers.rs:321`): desce
  para o core com `groups::model` (Fix A), conferindo antes o que `model` usa.
- `accounts`: trait `CodexLauncher` no crate da voz, que devolve o `Command` do app-server já
  montado e os ids das contas visíveis; implementado no servidor (Fix B). `catalog::Account` não
  entra na interface.
- Rótulo do servidor: `Arc<dyn Fn() -> String + Send + Sync>` no estado da voz.
- `voice::test_support` é `#[cfg(test)]` e só a voz o usa: vai junto, sem feature.
- O `VoiceHub` passa a receber o estado pelo padrão da Task 5.

- [ ] **Step 1: Conferir as referências** `grep -rnE 'crate::' crates/hangar-server/src/voice` fora de `crate::voice`, e os campos do `AppState` que a voz lê.
- [ ] **Step 2: Commit de corte** (Fix A e Fix B acima).
- [ ] **Step 3: Mover `voice/`** para `crates/hangar-voice/`.
- [ ] **Step 4: Provas comuns** (quando autorizadas).
- [ ] **Step 5: Uso real — verificação manual** chamada de voz no nativo: falar, ouvir a resposta,
  trocar de sessão por voz, consultar outra máquina.

### Task 8: Extrair `hangar-transcript`

Status: ready-for-agent (depois da Task 7)

`transcript` (3.665 linhas) é lido por 45 módulos. Depois das Tasks 2 e 3 restam duas ligações para
cima: `orchestration::timeline::parse_obj` (`transcript/history.rs:399`, `transcript/mod.rs:131`,
para `Provider::Orq`) e `runtime::queue::row_waits` (`history.rs:584`, core pela Task 3).

**Revisto em 07/11/2026 (conferido no código).** O Fix A não se paga: `timeline` usa também
`panel::panel` e `triage::decide`, e `runs` usa vários ajudantes de `orchestration/common.rs`; o
`panel` puxa `costs::collect::Collector`, `consumption`, `plan` e `snapshot` (metade da orquestração
e os custos). Fora do core, o `transcript/` só depende de `warn_limit` e `row_waits` (já no core) e
de `orchestration::timeline::parse_obj`, chamado em dois lugares: `LineParser::feed`
(`transcript/mod.rs:125`) e o histórico (`transcript/history.rs:399`).

**Decidido (Jefferson, 07/11/2026): a função de leitura do `orq` entra por registro na subida.** O
`hangar-transcript` guarda `OnceLock<fn(&Value, Option<&Path>) -> Vec<ChatEvent>>` com um
`set_orq_parser` público; os dois pontos chamam pelo registro. O servidor registra
`orchestration::timeline::parse_obj` antes de servir e em todo ponto de entrada que lê transcript
(subida em `lib.rs`, comandos do `main.rs` que leem conversa, e o ajudante comum dos testes que
montam o estado). Sem registro, a leitura de `orq` loga erro uma vez (código
`transcript_orq_parser_missing`) e devolve vazio; um teste do servidor confere que, depois da
subida, uma linha de `orq` gera eventos. Os testes de `transcript/orq_cases.rs` usam a orquestração:
ficam no servidor.

- [x] **Step 1: Conferir as dependências** de `transcript/` fora do core (esperado: só
  `orchestration::timeline::parse_obj`) e listar todo ponto de entrada que lê transcript sem passar
  pela subida do servidor (`main.rs`, exemplos, testes).
  Feito: só `parse_obj` (o `row_waits` já é re-export do core). Nenhum subcomando do `main.rs` lê
  conversa de `orq`. Pontos de entrada: `AppState::with_parts` (todo construtor do estado passa
  por ele: subida e testes que montam estado), o exemplo `orq_contract` (`timeline.history`) e o
  teste `transcript::orq_contract`, que não monta estado. `bench_history` só lê Claude.
- [x] **Step 2: Commit de corte** com o registro (`set_orq_parser`/`orq_parser`) ainda dentro do
  servidor, as duas chamadas trocadas e o registro feito em cada ponto de entrada do Step 1.
  Feito: `transcript::{OrqParser, set_orq_parser}` e `parse_orq` (sem registro: `tracing::error!`
  uma vez com `transcript_orq_parser_missing`, devolve vazio). RED: os testes
  `routes::tests::orchestration_history_uses_the_same_parser_on_public_and_forwarded_ports` e
  `transcript::orq_contract::…` falharam antes do registro; GREEN depois.
- [x] **Step 3: Mover `transcript/`** para `crates/hangar-transcript/` pelo contrato.
  Feito: `mod.rs` virou `lib.rs`; `claude`, `codex`, `history` e `peer` foram junto; o servidor
  re-exporta com `pub use hangar_transcript as transcript`. `orq_cases.rs` e o teste de contrato do
  `orq` foram para `orchestration/orq_cases.rs` (usam a orquestração); no crate ficou um teste de
  que, sem registro, a linha de `orq` não gera evento. Viraram `pub` só os itens que o servidor usa
  (`history` e 16 itens dele, `RewriteFilter` com `keep`/`to_state`/`from_state`, os ajudantes de
  `claude`/`codex`, `event`, `finish`, `parse_claude_object`, `py`, `json_tree`). `memchr` passou a
  dependência do workspace, usada pelos dois crates.
- [x] **Step 4: Provas comuns** (quando autorizadas).
  Feito: `check -p hangar-server --all-targets` sem aviso nos arquivos tocados; `test -p
  hangar-transcript` 37/37; testes internos do servidor 2769 ok (20 ignorados); `it` 3259 ok (21
  ignorados); `cargo metadata` sem aresta para o servidor. Código só de outra plataforma: o ramo
  `not(target_os = "linux")` de `peer.rs` não importa nada; o passo Windows fica com o CI.
- [ ] **Step 5: Uso real — verificação manual** abrir conversas Claude, Codex e uma sessão `orq`;
  histórico, painel da orquestração e eventos ao vivo aparecem.

### Task 9: Extrair `hangar-omp-sync`

Status: ready-for-agent (corte decidido no Step 1; não espera as Tasks 4 a 7)

`omp_sync` (5.003 linhas) usa: `accounts::secondary_auth::{omp_directories, OmpDirectories}` (core
pela Task 3), `install_root::root`, `runtime::process::resolve_program` e `atomic_write` (core pela
Task 3), `session_write::json_response` e `transcript::pyjson` (core pela Task 2),
`loops::automations` (recebe `&AppState`, `loops/mod.rs:36`), `nav_requests::now`,
`push::server_env` e `routes::AppState` (padrão da Task 5).

- [x] **Step 1: Decidir o corte de `automations`, `now` e `server_env`**: Fix A se for função pura
  ou leitura de caminho; Fix B (trait ou função no estado) se ler o `AppState`.
  Decidido (07/11/2026, conferido no código): o `AppState` só entra em `Service::start` e no
  `serve` (que não o usa). `start` fica no servidor e chama `Service::launch(env, home,
  claude_base, template, allowed)` do crate: `push::server_env()`, os dois caminhos da conta, o
  modelo de contexto (`install_root::root`, saindo de `context_template`) e o fecho `allowed`
  (`loops::automations` + `diag.report`) são montados pelo servidor (Fix B). `serve` perde o
  parâmetro do estado. Fix A: `nav_requests::now` → core (cópia de `state::monitor::wall_now`,
  `queue_feed::wall_now` e `list/bridge.rs:287`, que ficam), `runtime::process::resolve_program`
  → `core::process`, `list::links::without_verbatim` (só testes) → `core::fs`. `mechanism_tests.rs`
  monta `AppState` e roteador: fica no servidor. Com isso a Task 9 não depende da Task 5 nem do
  `install_root`.
- [x] **Step 2: Commit de corte.**
  Feito: `fs::without_verbatim`, `process::resolve_program` e `clock::wall_now` (novo) no core;
  `nav_requests.rs` intocado (a `connect-rust` o reescreve), então `nav_requests::now` segue como
  cópia. `omp_sync/host.rs` guarda `start` (monta env, caminhos, modelo e o fecho `allowed`) e
  `serve`, mais os testes `op099` (monta `AppState`) e o de `install_root` do `op080`; o
  `routes.rs` muda uma linha. Provas: `check -p hangar-server --all-targets` limpo;
  `test -p hangar-core` 17/17; servidor `omp_sync`/`list::links`/`runtime::process` 167 ok.
- [x] **Step 3: Mover `omp_sync/`** para `crates/hangar-omp-sync/`.
  Feito: tudo do `omp_sync/` foi (código, contratos `.json`, geradores `.py`), menos o `host.rs`, que
  virou `src/omp_sync.rs` do servidor com `pub use hangar_omp_sync::*` (os caminhos antigos seguem).
  `serde-saphyr`, `base64` e `proptest` passaram a dependências do workspace. Os `sleep` em laço de
  `/proc` e o `home_dir` que só lê os binários nativos do omp ganharam `estavel:` (portão de testes).
- [x] **Step 4: Provas comuns** (quando autorizadas).
  Feito: `check` do crate e do servidor (`--all-targets`) limpos; `test -p hangar-omp-sync` 154 ok
  (4 ignorados); testes internos do servidor 2615 ok (os 2769 de antes menos os 154 que saíram);
  `it` 3259 ok; `cargo metadata` sem aresta para o servidor.
- [ ] **Step 5: Uso real — verificação manual** sincronização omp: plugins e contexto reconciliados numa sessão omp.

### Task 10: Refazer o grafo e planejar a segunda fase

Status: ready-for-agent (depois da Task 9)

- [ ] **Step 1: Refazer o grafo de módulos** do que sobrou no `hangar-server`, com o mesmo método
  da seção "Números conferidos", e a lista de alvos restantes por módulo.
- [ ] **Step 2: Medir a memória** de um `rustc` do `hangar-server` e de cada crate novo (quando o
  Jefferson autorizar o build) e trocar a tabela de estimativa pelo medido.
- [ ] **Step 3: Escrever o plano da segunda fase** em `docs/superpowers/plans/`, na ordem da seção
  "Ordem e paralelismo" (revista pelos testes) e refazendo a contagem de "Testes por módulo":
  `interactions`, `terminal_input`, `term`, `terminal_facts`, `mods` e `plugin` (destravam o nó,
  cada um com o corte medido antes); a avaliação de
  `codex_setup` e `update`; o nó `runtime`/`accounts`/`list`/`session_open`/`session_write`/`state`
  (com a inversão da seção "Adiado"); depois `speech`, `remote_browser`, `orchestration` (com
  `orq_storage`), `uploads`, os pequenos (`pages`, `permission_control`, `subagents`, `tail`),
  `share` e `groups`; incluir o PR que retira os re-exports do item 1 do contrato.
- [ ] **Step 4: Atualizar a entrada de plataforma.md** com os números medidos.

## Adiado para a segunda fase

### Adiado: inverter `accounts` → `RuntimeRegistry` com trait (Fix B)

Decidido (Jefferson, 06/11/2026): fica para quando `accounts` ou `runtime` for sair do servidor,
na segunda fase; nesta fase o trait não libera crate nenhum.
Revisão: risco — contrato entre os dois maiores módulos.

Depois da Task 3, o que sobra de `accounts` → `runtime` é o `RuntimeRegistry`: guardado como campo
em `accounts/quotas/mod.rs:119` e `accounts/claude_refresh.rs:479`, recebido como parâmetro no resto
(`claude_refresh`, `codex_login`, `codex_device_login`, `usage`). Os métodos chamados são
`account_usage` (`accounts/usage.rs:379`, definido em `runtime/gateway.rs:2900`), os de
`device_guard` (`accounts/codex_device_login.rs:283`) e `refresh_hooks` (`accounts/http.rs:554`).

- `account_usage` e `refresh_hooks` recebem `accounts::AccountKey` e devolvem
  `accounts::UsageFacts` (`gateway.rs:2900,2907`): o trait `AccountRuntime` no core só existe se
  esses dois tipos descerem junto (Fix A).
- Os testes de `claude_refresh` montam um `RuntimeRegistry::new` real (`claude_refresh.rs:639,696`):
  continuam valendo, porque o `RuntimeRegistry` implementa o trait.
- Por que foi adiado: `accounts` ainda tem ~165 `crate::` em 25 arquivos e não sai do
  servidor nesta fase, então o trait não destrava crate nenhum agora.

- Inventariar os métodos do `RuntimeRegistry` que `accounts` chama e os tipos deles.
- Commit Fix A de `AccountKey` e `UsageFacts` (e do que eles arrastarem) para o core.
- Commit Fix B com o trait, a implementação no `runtime` e a troca dos campos e parâmetros em `accounts`.
- Conferir o corte: `grep -rn 'crate::runtime' crates/hangar-server/src/accounts`
  fora de teste → vazio.
- Provas comuns (quando autorizadas).
- Uso real — verificação manual cota das contas Claude e Codex aparece; login
  Codex por aparelho conclui; sessão que bate limite troca de conta.

## Aceitação desta fase

- `hangar-core`, `hangar-costs`, `hangar-voice`, `hangar-transcript` e `hangar-omp-sync` existem;
  nenhum depende do `hangar-server` (conferência por `cargo metadata`).
- `costs_routes` e os crates extraídos não citam `AppState`.
- Os testes do servidor não gravam no diário real.
- Cada Task foi um PR com as provas comuns verdes, o passo Windows e o uso real conferido.
- A memória medida nas Tasks 1 e 10 substitui a estimativa.

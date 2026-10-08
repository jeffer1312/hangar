# Parte 5, metade Claude: spec

Base: `analise.md` (levantamento com arquivo:linha sobre `dcae49227`, contrato 36). Parceira:
metade Codex em `../parte5-codex/spec.md` (subpartes 5A–5E dela). O combinado entre as duas está
em `contrato-par.md`. Nada disto foi implementado; o plano só começa depois da aprovação do dono.

## A regra, em uma frase

Com o Rust de pé, **toda escrita numa sessão Claude entra pelo Rust e é executada por ele, sem
volta ao Python no caminho**: a rota, a validação, a fila, o teclado do pane, o plugin e os fatos
que a entrega consulta. O Python fica com o que é da parte 6 (troca de conta, convidado,
Connect, par, convites e peers). Contas e cotas Claude/Codex foram separadas na #115;
o Python responde só por fatos e ganchos delimitados dessas áreas, nunca por
um passo da escrita. Falha vira erro com código; só a reserva do processo inteiro (modo `python`)
roda o código atual.

## Fora desta metade

- **Codex** (todas as rotas e serviços só dele): metade Codex.
- **Criar sessão (`POST /api/sessions`)**: parte 6 (decisão 1, abaixo).
- Contas, login, identidade e cotas Claude/Codex: [issue #115](https://github.com/jeffer1312/hangar/issues/115),
  com contratos e autoria no [inventário](../../../backend/tests/fixtures/accounts_contract/README.md).
  A preparação Claude continua no reconciliador Python por gancho restrito até sua migração;
  a #115 decide a operação e mantém a proteção de existência da conta.
- Troca de conta (`/conta`), convidado e Connect (8766/8768), par e grupo (`/pair*`,
  `/bastao`, `/group-message`), `/then`, `/loop`, orq, MCP, push, atualização: parte 6 ou
  provedores próprios. Esses caminhos continuam chamando `coordinator.op` do Python, que segue
  existindo como cliente interno do Rust.
- Remover o código Python que ficar sem uso no modo `rust`: parte 7 (o modo `python` ainda o roda).

## Decisões

1. **Criar sessão vai para a parte 6.** Quase tudo que a criação faz é da parte 6: resolver
   conta e `config_dir`, o ambiente do motor (`engines.py`),
   criar worktree (mutações de worktree estão na parte 6) e a raiz do convidado. A parte que é da
   sessão já está no Rust: o vínculo nasce lá (`_await_birth`, `ensure_open`). Portar a criação
   agora traria metade da parte 6 junto. A #115 troca a proteção de existência por um protocolo
   de lock compartilhado Python/Rust, sem portar o nascimento da sessão; a criação segura o
   descritor até comprovar o processo, inclusive após cancelamento HTTP. *Aprovado pelo dono em 07/10/2026 (o pedido original
   listava a criação no escopo).*
2. **`model_picker.py`: parte 5** (subparte C4). É o que obriga o empréstimo do teclado no
   `/model-effort`.
3. **Motores (`engines.py`): parte 6.** Só entram na criação e no relançamento. A C4 lê
   `engines.json` só para validar o id do `/engine/model`.
4. **Hooks e marcadores: a memória do `hook_state`, o rebaixamento (`/internal/list/demote`) e a
   drenagem disparada pela transição vão para a parte 5** (C3). O `hook_installer` (grava
   `settings.json` das contas) vai para a parte 6.
5. **Ponte de skills (`skill_bridge.py`): parte 6.** Serve Pi, Kimi e Codex; não está no caminho
   de escrita do Claude.
6. **Troca de conta em andamento vira barreira do Rust.** Hoje `_transfer_guard` e
   `session_ingress` seguram a escrita no Python. Com a rota no Rust, o dono da escrita é quem
   fecha a porta: a troca de conta (Python, parte 6) pede ao Rust para fechar a entrada da sessão
   (espera as escritas em curso, recusa as novas com o mesmo 409 `session_transfer_busy` de hoje, `api.py:569`) e reabre no fim.
   Uma chamada por troca, nenhuma por escrita.
7. **Convidado e Connect continuam entrando pelo Python** (portas 8766/8768, parte 6) e chegam ao
   Rust por `coordinator.op`, como hoje. As rotas novas do Rust atendem o dono na 8765.
8. **Sessão Claude não gerenciada com o Rust de pé é erro com código**, nunca o caminho antigo do
   Python (socket nativo, plugin ou tmux pelo Python). Regra do dono único.
9. **Ramos mortos saem já na 5-0:** `answer_body`, `session.marker`, `diag.error` e `quota` do
   `runtime_policy.py` não têm chamador (análise, seção 4).

## Subpartes e ordem

```
5-0 caminho de escrita (comum) ──► C1 serviços restantes ─┐
                              ├──► C2 plugin no Rust ──────┼─► C3 fatos do terminal ─► C4 administração ─► C5 ciclo de vida
                              └──► (metade Codex: 5B, 5C…) ┘
```

Cada subparte é uma junção na `main` e sobe o contrato interno uma vez.

### 5-0 Caminho de escrita (comum aos dois provedores; feita e revisada por esta metade)

- **Tabela de despacho no Rust** (`rs/session_write/`): para cada rota, provedor e modo da sessão
  (com/sem terminal), `Rust` ou `Python`. O Rust reivindica a rota em `routes::router` e em
  `migration_status::rust_route` e repassa ao Python as linhas marcadas `Python`. A metade Codex
  vira as linhas dela trocando a tabela, sem mexer no resto. `enum Provider { Claude, Codex }`
  sobre o ator que já existe; sem trait (são dois e o ator é o ponto comum).
- **Rotas da 5-0 para Claude:** `/input`, `/steer`, `/interrupt`, `/select`, `/select/submit`,
  `/answer`, `/keys`, `/term-input`, `DELETE …/queue/{entry_id}`. Codex: repassadas até a 5B/5C
  dela.
- **Validação portada**: sessão existe (404), provedor não migrado e orq (repasse pela tabela),
  painel do terminal aberto (`term.active`, já no Rust), `perm:` só opção 1/2, barreira de troca
  de conta (decisão 6), formato de erro `erro(código, mensagem)` igual ao do Python.
- **Resposta por chat** (`/answer` com `kind=chat`) sai do empréstimo: Esc pelo `interrupt()` do
  Rust, espera do rodapé pelo regex que o Rust já tem, texto pela fila.
- **`/interrupt` avisa o plugin** por um serviço interno temporário (`plugin.interrupted`), uma
  chamada por interrupção; sai na C2.
- **`runtime_coordinator`**: `_born_in_rust` e os testes de `provider == "claude"` (`rc:515`,
  `:1026`, `:1248`) passam a perguntar à tabela se o Rust é dono do provedor e do modo. `op`
  continua como cliente interno do Rust para quem nasce no Python (decisão 7 e "Fora").
- **Serviços portados na 5-0** (os que os dois provedores usam): `prepare_prompt` (chamado em
  cada Input/Steer), `format_status` (texto dos dois provedores) e `skill_catalog`. As janelas de
  cota do Claude o Rust pede ao Python (`GET /internal/quota`) só quando vai formatar e o cache
  dele, de 5 min, venceu. Na #115, ambas as cotas passam ao próprio serviço Rust e essa leitura
  Python sai do caminho; as cotas dos outros provedores continuam por gancho restrito.
  O texto do Codex bate por golden com `format_status_line`. O Rust não pede mais
  o texto da linha ao Python.
- **Golden** de cada rota: corpo de resposta e de erro idênticos ao do Python, gerados pelas
  rotas Python (`backend/tests/fixtures/contract/`).

### C1 Serviços restantes do ator

`last_usage`, `reload_stamp` (hash idêntico ao `_marca_config`: `json.dumps(sort_keys=True)` e
junção com `\0`), `unknown_private` (mesmos tetos e arquivos), `session.patch_meta` (o Rust grava
o sidecar; o Python passa a só ler) e `native_message` com caixa de entrada própria do Rust (o
`from=uds:` hoje aponta para o socket do Python; o plano confere quem lê as respostas). Depois da
C1, `/internal/runtime/policy` só atende o que a metade Codex ainda não tirou.

### C2 Plugin do Claude no Rust

- Todas as rotas `/api/plugin/*` que faltam (`whoami`, `pull`, `suggest`, `ask`, `ask-fim`,
  `filled`, `submitted`, `state`, `rate`) e a memória delas no Rust, com o mesmo token
  (`mods/bridge.rs:57`) e os mesmos prazos (25 s, 5 s, 30 s, 35 s, 90 s).
- Entrega pelo plugin, permissão segurada e `AskUserQuestion` segurado no Rust: somem
  `terminal_publish`, `terminal_plugin_control`, `plugin.interrupted` e a parte do plugin do
  `state-facts`.
- Quem lê essa memória no Python e continua lá (porta do convidado em `sse.py`, `list_facts`
  `held`) passa a ler do Rust pelo canal privado que a parte 4 abriu.
- Conserto junto: o fato `question` passa a respeitar `interrompida` e o teto sem poll, como o
  `pergunta_pendente` do Python; e a regra `harnesses.md:381` é corrigida para o valor do código
  (5 s) ou o código para a regra — o plano decide com a medição.

### C3 Fatos do terminal e serviços de estado

- `terminal_facts` no Rust (composer, classificação, pergunta, plugin, marcador, socket nativo,
  clipboard do Windows): some a chamada ao Python antes de cada entrega.
- `permission.observe` com a memória por sessão no Rust; `session.dead` no Rust (o `em_troca`
  segue como fato empurrado pelo Python, parte 6); `session.deliverable` chama o `drain_once` do
  próprio Rust em vez da ida e volta pelo adaptador Python.
- Memória do `hook_state`, rebaixamento e drenagem pela transição no Rust; sai
  `/internal/list/demote`.

### C4 Administração do terminal sem empréstimo

- `/model-effort`, `/model/options`, `/engine/model`, `/permission-mode` (troca e lista) e `/btw`
  executados pelo Rust no pane, com o `model_picker` portado (fixtures existentes viram golden) e
  os buffers do psmux no Windows.
- Paridade da resposta por opção: atalho de dígito e Enter conferido com painel de prévia.
- Troca de motor com esforço numa operação só (hoje são dois empréstimos e a fila pode drenar
  entre eles).
- O empréstimo de teclado sai dos dois lados (`rt:427-473`, `terminal.rs:253-290`). Some junto o
  defeito provável do `/btw` estourando o prazo do empréstimo.

### C5 Ciclo de vida

`/rename`, `DELETE`, `/recarregar`, `/modo-execucao`, `/open-terminal` e `/resume` no Rust. O que
é da parte 6 (par, convite, aviso aos peers, atalhos) roda depois, por um gancho interno do Python
(`session.after_close`, `session.after_rename`) que não bloqueia a resposta. O `DELETE` passa a
fechar o vínculo do terminal no Rust na hora, em vez de esperar o `Monitor` notar o pane sumido.

## Desempenho

Conferido contra "Desempenho: erros que já custaram" do `README.md`. Ganhos esperados, medidos
antes e depois em release, backend isolado:

- Cada `/input` deixa de fazer Rust → Python → Rust (rota) e Rust → Python (`prepare_prompt`).
- Cada entrega com terminal deixa de pedir `terminal_facts` ao Python.
- Cada sessão sem terminal deixa de pedir `reload_stamp` a cada 10 s.
- Medida: chamadas a `/internal/*` por minuto com 5 sessões Claude ativas (com e sem terminal),
  latência do `/input` do pedido ao `delivered`, CPU do Python parado e pico de RSS do Rust.

Nada de laço novo: a memória do plugin acorda por evento, não por varredura; fatos empurrados só
quando mudam.

## Prova

- Golden de cada rota e serviço portado contra o Python.
- Testes do Rust para tabela de despacho, barreira de troca de conta e plugin (long-poll,
  segurar, soltar, restart).
- **Uso real no backend do notebook do dono, pelo canal de testes, feito por ele com a sessão
  `hangar`.** Esta metade não sobe nada no backend do notebook: avisa quando cada junção estiver
  pronta para testar. Roteiro mínimo por subparte: mandar, interromper, responder pergunta por
  opção e por chat, permissão segurada no celular, `/model` e esforço, `/btw`, renomear e fechar,
  restart do backend com sessão viva.
- Windows: CI por job; `/btw` e o seletor `/model` na VM DELPHI-02 quando o dono liberar.

## Riscos

- **Plugin com sessões vivas na troca de versão.** O plugin reconecta pelo token determinístico,
  mas a memória (pergunta segurada, publicação em curso) se perde na troca de dono. Pergunta
  segurada no momento do update cai para o diálogo do terminal, como num restart de hoje.
- **Leitores Python do sidecar** (`registry`, `_classe_modo`) depois que o Rust passa a gravá-lo
  (C1): gravação atômica e o mesmo formato; o plano lista cada leitor.
- **Caixa de entrada do `native_message`** (C1): quem consome hoje as respostas no Python precisa
  de um equivalente antes de trocar a origem.
- **Conflito com a metade Codex** nos mesmos arquivos (`runtime_policy.py`, `internal_api.py`,
  `routes.rs`, `migration_status.rs`, número do contrato): regras em `contrato-par.md`.

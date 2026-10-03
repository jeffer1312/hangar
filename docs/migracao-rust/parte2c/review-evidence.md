# Parte 2C — evidência das revisões


## task-1-report.md

# Task 1 — resultado da implementação

Port completo dos parsers puros e do reducer de terminal, com comparação campo a campo contra Python. Sem integração em produtor, endpoint, captura ou sessão viva nesta Task.

Commit seletivo: `d23d59f`, `feat(server): port terminal state and preview reducers`, branch `hangar-server-parte2c`. `git status --short` limpo após o commit. Plano e relatório estão no disco e não são arquivos versionados neste checkout.

## Arquivos

- `crates/hangar-server/src/terminal_state.rs`: tipos serde com defaults, `analyze` e `reduce`; menus numerados/sem número, guardas de scrollback/rascunho, spinner, statusline, overlay, login, limite, seletor Codex e prévia extraída exclusivamente como Claude.
- `crates/hangar-server/src/lib.rs`: somente export do módulo.
- `crates/hangar-server/tests/contract_terminal.rs`: comparação JSON completa das análises e das sequências com memória reaproveitada.
- `backend/tests/fixtures/contract/gen_terminal.py`: geração usando funções Python originais e o `StateMonitor.stream` original. Fontes externas são substituídas por entradas em memória; o script observa os locais do monitor na captura seguinte, depois da atualização da memória, inclusive quando não houve emissão.
- `backend/tests/fixtures/contract/golden/terminal.json`: resultado Python versionado.
- Plano: Steps 1–4 da Task 1 concluídos; Step 5 permanece para revisão independente.

## Provas e comandos

1. `uv run --directory backend python tests/fixtures/contract/gen_terminal.py`: gerador executado. A primeira tentativa expôs que um plugin sem label de opção causa `ValidationError` no `StateEvent` Python. Esse dado inválido foi retirado; os golden usam o contrato válido das perguntas. O uv avisou que ignorou o `VIRTUAL_ENV` de outro checkout e criou a `.venv` deste checkout.
2. `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_terminal`, antes de escrever produção: saiu 101, `E0432`, módulo `hangar_server::terminal_state` ausente. Falha esperada.
3. Mesmo comando, após o port inicial: 1 teste passou.
4. Ampliação dos casos Unicode antes da correção: mesmo comando saiu 101, caso `unicode_numbering`. O Python reconheceu as opções com `١`/`٢`; o Rust devolveu `codex_menu: null`, porque `parse::<usize>()` não aceita esses dígitos. A conversão agora segue os blocos decimais Unicode, sem cortar texto por byte. As classes de espaço/palavra e os limites de palavras também seguem o Python, incluindo marcas combinantes e separadores de informação.
5. Mesmo comando, após a correção: 1 teste passou, 0 falhas, aproximadamente 0,05 s de teste.
6. `git diff --check`: passou.

Metodologia de contagem: o JSON contém 64 objetos, separados pela presença de `pane` ou `sequence`: 48 análises estáticas (os 19 `pane_*.txt` existentes mais 29 sintéticos) e 16 sequências com 65 frames. Cada campo de saída, inclusive memória, é comparado ao resultado original Python.

Cobertura temporal: quatro quadros com spinner igual; quatro ausências após working; hook com grace 8 e sem expiração; menu imediato; plugin idle ignorado durante animação; hook idle ignorado durante animação e aplicado quando congela; plugin seguido de hook e seus resets; statusline inteira e vazia; pergunta de hook fora do pane; pedido `perm:*`; menu visível conservado; pergunta de plugin vazia e troca sem reticências.

Não rodaram suíte inteira, pytest, check, build de front, instalação, restart ou terminal do usuário. O comando focado Rust e a geração de referência Python foram autorizados para esta Task. Uso real é responsabilidade das Tasks de captura/integração.

## Interface para Task 3

```text
analyze(pane: &str) -> PaneAnalysis
reduce(pane: &str, memory: ReducerMemory, facts: ReducerFacts) -> ReducedState
ReducedState = { analysis, memory }
```

`PaneAnalysis`: `state`, `label`, `question`, `options`, `spinner`, `status_line`, `overlay`, `login`, `limit_reset`, `preview`, `codex_menu`. `options` é `Option<Vec<String>>`; `codex_menu` é `{question: Option<String>, options: Vec<String>}` ou null. `limited` deriva de `limit_reset.is_some()` no produtor.

`ReducerMemory`: `prev_spinner`, `frozen`, `no_spinner`, `held_state`, `held_label`; nasce idle, contadores zero e valores opcionais nulos. Recebe/devolve toda a memória; não depende de relógio ou IO.

`ReducerFacts`: `open_question` normalizado como `TerminalQuestion`; `plugin_question` é o pedido original em `serde_json::Value` (`id`, `tool`, `resumo`, `questions`); `plugin_state` e `hook_state` são strings opcionais; `hook_grace` nasce `Some(8)`, null significa sem expiração; `status_line` é string opcional. Fatos ausentes usam defaults. Statusline vazia cai no pane, igual ao `or` Python.

A ordem é a do monitor original: classificar pane → pergunta de hook fora da tela → pergunta de plugin → spinner/debounce → plugin (com reset dos contadores) → hook → statusline. Não há precedência nativa nova nem adaptação de `waiting`.

`held_state`/`held_label` são guardados em todo retorno: quando o Python não emite, esses dois campos já eram iguais aos que ele guardou; a sequência original verifica essa equivalência. O estado é limitado pelo produtor original; pane vazio continua análise idle, mas erros de captura não devem ser convertidos para pane vazio na integração.

## Limites e observações para revisão

- Os fatos de pergunta do plugin pressupõem o contrato válido atual: labels são strings. Dados malformados, como label ausente, já falham no `StateEvent` Python; a integração deve validar esse contrato antes de reduzir. O port não cria recuperação para dados inválidos.
- O gerador usa introspecção dos locais do monitor de referência. Uma renomeação no Python exige atualizar o gerador; ele falha com chave ausente, sem produzir golden silenciosamente diferente.
- Fixtures Pi/omp/Kimi só comprovam o parser genérico e a extração com `provider="claude"`. Não se adicionou runtime desses providers no Rust.
- Nenhum log recebe pane ou texto de conversa.

## Correção da revisão — rodada 1

Escopo: somente as duas diferenças Unicode apontadas pelo revisor, sem alteração da ordem das fontes ou dos contadores.

Evidência direta do Python antes da edição: `extract_assistant_text` devolveu vazio para `Running` seguido de U+0345 e `Calling` seguido de U+05B0; preservou apenas `Resposta` antes do aviso `Agent "worker" finished` com U+0345 e antes de um resumo `ran 2 shell commands` precedido por U+05B0. `is_login("Select logın method")` e a variante com `İ` devolveram true; `rate_limit_reset` reconheceu `9:10pm` nas frases de limite com ambas as formas de i. Os casos comuns `Select LOGIN method` e `ſelect login method` também deram true.

- Causa 1: `char::is_alphanumeric()` usa a propriedade Unicode Alphabetic, que inclui certas marcas combinantes; `re \w` do Python usa as categorias L/N e underscore. O helper `word` agora usa a mesma classe `^[\p{L}\p{N}_]$` já aplicada nas regex. Isso corrige os limites de palavra em ferramenta, MCP, aviso de agente e resumo de atividade.
- Causa 2: o ignorecase Rust não reúne `i` com `ı` e `İ` como `re.I` Python. As duas regex case-insensitive atuais (login e limite) agora expandem o i literal para `[iIıİ]`, preservando o ignorecase normal das demais letras.

Prova vermelha: depois de acrescentar dez entradas sintéticas e regenerar os golden com `uv run --directory backend python tests/fixtures/contract/gen_terminal.py`, `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_terminal` saiu 101. O teste passou a listar todas as divergências estáticas numa execução e mostrou oito diferenças: quatro na prévia, duas no login e duas no limite. Os outros dois casos servem para preservar os matches comuns.

Prova verde: mesmo comando após a correção passou: 1 teste, 0 falhas, aproximadamente 0,06 s. `git diff --check` passou. Metodologia atual: 74 objetos no golden, sendo 58 análises estáticas (19 fixtures existentes e 39 sintéticas) e as mesmas 16 sequências/65 frames. Nenhuma suíte inteira, formatter, serviço ou terminal real foi executado.

Arquivos da correção: `terminal_state.rs`, `contract_terminal.rs`, `gen_terminal.py` e `golden/terminal.json`. Interfaces públicas permanecem iguais.

Commit seletivo da correção: `3c14bd0e58afde47f4970970bcf55acf3f507fe2`, `fix(server): match Python terminal Unicode rules`, branch `hangar-server-parte2c`; `git status --short` limpo após o commit. A revisão da rodada 1 deve comparar com `d23d59fd7541a4f73f1a2d6e92ca71de81ae1271`.


## task-1-review.md

# Revisão independente — Task 1

**Veredito vigente após a rodada 1: aderência aprovada e qualidade aprovada. Os dois pontos foram consertados e conferidos em `3c14bd0e`.** A seção final registra a decisão atual; as seções anteriores preservam os achados da primeira rodada.

## Aderência ao combinado

**Precisa de correção.** O escopo, as interfaces e a ordem temporal estão atendidos; a paridade Unicode ainda diverge da referência Python em `crates/hangar-server/src/terminal_state.rs:134` e nas regex das linhas 108 e 110.

Os cinco arquivos previstos estão no diff. Não há captura, integração, runtime de outro provider nem logs de pane nesta Task. Sidecar de prévia, captura, publicação e integração ficam para as próximas Tasks; não são pendências desta entrega.

## Pontos bem feitos

- Tipos com defaults e memória explícita separam análise de redução (`terminal_state.rs:41`, `:57`, `:309`, `:338`).
- Menu imediato, congelamento 3, ausência 4, hook com grace 8 e statusline não vazia preservam a ordem do monitor original (`terminal_state.rs:356`, `:365`, `:370`, `:375`, `:384`, `:390`).
- A prova compara todos os campos e reaproveita a memória entre frames (`contract_terminal.rs:12`, `:18`). O gerador usa o monitor Python original e observa a memória após o processamento, inclusive sem emissão (`gen_terminal.py:35`, `:45`).
- Contagem conferida no JSON do diff: 48 análises estáticas e 16 sequências contendo 65 frames; entram todos os `pane_*.txt` (`gen_terminal.py:91`).

## Problemas

### Críticos

Nenhum.

### Importantes

**1. A fronteira de palavra muda a eleição e o corte da prévia.** `crates/hangar-server/src/terminal_state.rs:134` usa `char::is_alphanumeric()`, que considera a propriedade Unicode `Alphabetic`, incluindo marcas como U+0345 e U+05B0. O `\w` do Python considera letras/números/underscore e não essas marcas. Os helpers das linhas 135–146 passam essa diferença às decisões das linhas 292 e 300.

Reprodução exata, com escapes Unicode:

```text
● Resposta anterior\n● Running\u0345 texto\n  continuação
```

O Python retorna `preview = "Resposta anterior"`: reconhece `Running` como status de ferramenta. No Rust, `word('\u{0345}')` retorna true, `tool` rejeita a fronteira e o segundo bloco é eleito como prosa, devolvendo `"Running\u0345 texto\n  continuação"`. U+05B0 reproduz o mesmo efeito. Sem o segundo `●`, a linha de ferramenta também deixa de cortar a continuação.

Corrigir o helper para usar a mesma classe de palavra já adotada na montagem das regex (`[\p{L}\p{N}_]`) e acrescentar esses casos aos golden. O caso atual com U+0301 (`gen_terminal.py:118`) não cobre marcas com `Other_Alphabetic`.

### Menores

**2. A busca sem distinguir maiúsculas não tem a semântica Unicode do Python.** `crates/hangar-server/src/terminal_state.rs:108` e `:110` usam `(?i)` do Rust, baseado no case folding simples; `re.I` do Python também reconhece o “ı” sem ponto como “i”. Exemplos exatos:

```text
Select logın method
Usage lımit reached resets 9:10pm
```

Python: `login = true` no primeiro; `limit_reset = "9:10pm"` no segundo. As regex Rust não casam, devolvendo false/null. O impacto é pequeno porque os rótulos normais da TUI são ASCII, mas contraria a paridade pedida. Preservar a equivalência de `re.I` nessas regex e registrar os casos na referência.

## Evidência e limites

Risco conferido: paridade dos parsers e da memória temporal. Comparação direcionada com `backend/app/state.py:77–420`, `:882–1030` e `backend/app/preview.py:290–379`; nenhuma exploração de integração.

Sondas focadas chamaram as funções Python originais nos exemplos acima e confirmaram as saídas. A fonte local de `core/src/char/methods.rs:1050` confirma que `is_alphanumeric` usa `Alphabetic || N`; as tabelas e sondas do motor Rust regex confirmaram as propriedades das duas marcas e a diferença de case folding. A saída da prévia Rust acima foi derivada das condições do diff, sem executar `analyze` com esses novos casos.

Uma tentativa de compilar uma sonda em memória falhou porque o rustc tentou criar um diretório temporário ao lado do descritor `/proc/.../fd`; ela não produziu validação de execução. Não foram reexecutados os testes relatados, suíte, serviços ou tmux. O relatório da implementação expõe o aviso de `VIRTUAL_ENV` do uv; esse aviso não demonstra falha nos testes Rust.

**Qualidade da Task: precisa de correção.** A estrutura e a cobertura temporal são adequadas. Corrigir a fronteira Unicode da prévia antes de confiar na paridade; aproveitar a mesma rodada focada para fechar a diferença das regex de login/limite.

---

# Rodada 1 — decisão após a correção

**Aderência ao combinado: aprovada. Qualidade da Task: aprovada.** Esta decisão substitui os veredictos anteriores. Revisão focada no pacote `d23d59fd..3c14bd0e`, nos dois pontos apontados e nas possíveis quebras introduzidas pela correção.

**Ponto 1 — consertado e conferido.** `crates/hangar-server/src/terminal_state.rs:135` aplica `^\w$` pelo construtor que traduz a classe para letras/números/underscore; `:143–145` consulta essa regex com o caractere codificado em buffer local. As marcas `Other_Alphabetic` deixam de ser consideradas palavra. Isso corrige os quatro usos existentes: ferramenta, MCP, aviso de agente e resumo de atividade. As quatro regressões constam de `backend/tests/fixtures/contract/gen_terminal.py:120–123` e dos golden Python: prévia vazia para os dois blocos de ferramenta e preservação de `Resposta` nos dois casos seguintes.

**Ponto 2 — consertado e conferido.** `crates/hangar-server/src/terminal_state.rs:96–102` expande os i literais para `[iIıİ]` antes de aplicar `(?i)` e o construtor existente. As duas regex atuais de login e limite usam esse caminho (`:116`, `:118`). Seus padrões não possuem i em escapes, nomes de propriedades ou sintaxe de flags; a expansão não altera a estrutura da regex. Os casos com ı/İ, LOGIN comum e ſ ficam registrados em `gen_terminal.py:124–129` e nos golden.

**Quebras novas no diff: nenhuma encontrada.** A memória, a ordem das fontes e as interfaces públicas não mudaram. A acumulação das divergências estáticas continua fazendo o teste falhar ao final (`crates/hangar-server/tests/contract_terminal.rs:15`, `:27`); as sequências mantêm comparação completa por frame.

**Evidência considerada:** o relatório da rodada registra oito divergências antes da correção e a prova focada passando depois, com 58 análises estáticas e 16 sequências/65 frames. São dez casos novos no diff: oito regressões e dois casos comuns de preservação. Não repeti teste, suíte, sonda, serviço ou tmux; não li código posterior ao pacote congelado. Nenhum problema crítico, importante ou menor permanece aberto nesta rodada.


## task-2-report.md

# Task 2 — controle tmux persistente

Implementada em `terminal_control.rs` e no filtro de presença de `plugin_bridge.py`. Somente estes arquivos e suas dependências/testes foram alterados; nenhuma sessão do usuário, serviço ou instalador foi operado.

Commit seletivo: `1f67623eaca52bd9b3da453156aab0a4204d65c2`, branch `hangar-server-parte2c`; `git status --short` vazio após o commit. Nenhum push.

## Evidência

- RED Rust inicial: `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test terminal_control` retornou 101 com E0432, módulo `terminal_control` ausente.
- RED Python: `(cd backend && uv run pytest tests/test_plugin_bridge.py -q)` teve 53 testes aprovados e 3 falhas novas nos casos de ausência/controle.
- RED adicional de protocolo: `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test terminal_control unsolicited_queued_frames_never_supply_a_successful_capture` reproduziu captura indevida de uma resposta extra enfileirada. O comando passou após o descarte de frames sem pedido.
- GREEN final: comando Rust focado acima, sem filtro, **16/16 aprovados**; comando Python focado acima, **61/61 aprovados**. Contagens são os casos executados nesses dois arquivos, inclusive parametrizações. `git diff --check` passou. Não foi executada a suíte inteira, conforme autorização.
- Uso real isolado: tmux 3.7b em socket temporário próprio, com configuração `/dev/null` e panes sintéticos. Duas fontes compartilharam o mesmo PID. Última liberação removeu e aguardou o cliente; sessão permaneceu viva, 120×30 e `HANGAR_PROBE=kept` preservados. Captura foi comparada literalmente com `capture-pane` para histórico, ANSI, join, linhas vazias, UTF-8 e corpo `%begin exemplo literal`. Resize externo, alternate screen, sessão numérica, pane de outra sessão, troca do alvo ativo e pane morto foram exercitados. Expiração ocorreu mesmo com saída a cada 10 ms.
- Processo fake provou startup/command timeout, EOF ativo, erro de comando, UTF-8 inválido, reap e ausência de qualquer comando além de `display-message`/`capture-pane`, inclusive após consulta ANSI DSR. Parser fragmentado confere identidade dos marcadores e limites. Grade real confere UTF-8 dividido, cor, cursor e alternate.

## Interface para Task 3

```rust
TerminalPool::new() // também Default; Clone compartilha o pool
TerminalPool::with_program(program: impl Into<PathBuf>, socket: Option<PathBuf>, limits: Limits)
async acquire(CaptureRequest) -> Result<(), TerminalError>
async capture(CaptureRequest) -> Result<CaptureResult, TerminalError>
async release(&str) -> Result<(), TerminalError>
```

`CaptureRequest` (Deserialize, deny_unknown_fields): `consumer: String`, `name: String`, `provider: String` (`claude`/`codex`), `binding: String`, `target: String` (`%N` ou `=name:...`), `started: f64` finito, `lines: u32` até 10.000, `colors: bool`, `join: bool`. Todos os campos são obrigatórios na struct; Task 3 pode aplicar seus defaults antes de montá-la.

`CaptureResult` (Serialize): `binding: String`, `started: f64` devolvido sem conversão de relógio, `text: String`, `analysis: terminal_state::PaneAnalysis`. `TerminalError(pub &'static str)` implementa Display/Error e jamais inclui saída de terminal.

`acquire` sobe e semeia o observador se ausente; chamadas posteriores renovam a lease sem captura. `capture` também adquire/renova. `release` desconhecido é no-op e nunca cria processo; sucesso da última liberação inclui kill/reap do filho, sem matar servidor/sessão. Troca de binding/provider/target do mesmo consumidor solta a referência antiga primeiro. Erro fatal fecha o actor e descarta a grade; tentativa posterior pode abrir outro observador. Ao soltar o pool, o fechamento dos canais também encerra os filhos.

## Limites e reserva

Default `Limits`: startup 3 s, comando 2 s, lease 90 s. Máximos: 64 actors, 256 consumidores no pool, fila de 32 comandos/actor, 4.096 eventos, frame 8 MiB e linha 1 MiB. Grade: até 65.536 células, dimensões até 1.024×512; sem scrollback próprio. A saída processada entre checkpoints tem teto de 8 MiB para também limitar sequências ANSI não terminadas. Exceder qualquer limite é erro, seguido da reserva Python; não publica pane vazio nem reutiliza sucesso anterior.

A grade recebe `%output` continuamente apenas do ID do pane validado. `VoidListener` descarta todos os eventos que escreveriam no PTY. Captura tmux continua autoritativa para histórico/wrap/blank/ANSI; a análise usa a grade apenas quando seu texto coincide exatamente com o quadro canônico. Checkpoints reposicionam linhas/cursor a partir de captura e metadados tipados. Alvo que muda de ID durante o actor é erro, sem trocar silenciosamente para outro pane.

Windows retorna `terminal control unavailable` sem executar tmux/psmux. O caminho foi protegido por cfg e há teste específico para Windows, mas não foi executado em Windows nesta máquina. Nenhum backend real foi reiniciado e nenhum fluxo em sessão do usuário foi conferido.

Documentação primária consultada: https://man.openbsd.org/tmux#CONTROL_MODE e https://docs.rs/alacritty_terminal/0.26.0/alacritty_terminal/.

Steps 1–4 marcados no plano e no combinado. Step 5 fica para a revisão independente do principal.

## Correções da primeira revisão

Dois problemas reproduzidos antes das correções:

- `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test terminal_control exact_full_line_without_newline_preserves_pending_wrap_cursor`: falhou com `invalid terminal cursor`. Pane em tmux privado teve exatamente 80 caracteres sem LF e metadados `pane_width=80`, `cursor_x=80`.
- `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test terminal_control late_fragmented_unsolicited_frame_never_serves_the_next_request`: falhou porque devolveu captura válida ao consumir um frame extra fragmentado emitido depois de receber o próximo comando.

Correção do cursor: coluna virtual igual à largura é aceita e traduzida explicitamente para última célula física mais `input_needs_wrap` do alacritty. O checkpoint restaura esse estado; teste da grade confirma que o próximo `Z` vai à linha seguinte e não sobrescreve a última célula.

Correção de correlação: parser conserva `FrameIdentity { timestamp, command, flags }`. Cada comando de consulta vai entre dois `display-message -p -l` com marcadores exclusivos por processo/comando. A resposta exige início correto, exatamente um bloco de corpo e fim correto, com identidades em ordem e flags iguais. Os contadores podem saltar; não existe premissa de incremento unitário. O corpo só é devolvido depois de conferir o fim. Frames atrasados ou fragmentados, fim ausente ou errado e erro de protocolo descartam o observador. Um fim atrasado é esperado, sem sucesso antecipado.

GREEN da rodada: `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test terminal_control`, **20/20 aprovados**, e `git diff --check` limpo. Apenas Rust foi alterado nesta correção; o resultado Python anterior permanece 61/61. Os ensaios reais continuam somente em sockets privados com panes sintéticos. Interface do pool/captura/liberação não mudou; o evento público do parser ganhou `identity`, e `Screen::restore_cursor` serve à restauração do checkpoint.

Commit da correção: `962fbe1`, branch `hangar-server-parte2c`, árvore limpa após o commit; sem push.


## task-2-review.md

# Task 2 — revisão independente

**Conformidade: aprovada. Qualidade: aprovada.** Os dois problemas importantes da revisão inicial foram consertados e conferidos na rodada `1f67623e..962fbe11`. Nenhum bloqueio permanece no escopo da Task 2. As referências históricas abaixo correspondem ao primeiro intervalo; a conferência final indica as linhas atuais.

## Pontos bem resolvidos

- `crates/hangar-server/src/terminal_control.rs:320`: processo sem shell, `-N`, alvo de sessão exato, `-E`, `read-only,ignore-size`, `kill_on_drop`; os comandos construídos em `info`, `seed` e `capture` são somente leitura. `VoidListener` em `:143` impede respostas ANSI ao PTY.
- `crates/hangar-server/src/terminal_control.rs:278`: entrada limitada e alvo validado; `PaneInfo::parse` confere a sessão, e `info` recusa substituição do pane. Captura devolve binding e instante original em `:406`.
- `crates/hangar-server/src/terminal_control.rs:389`: texto continua vindo de `capture-pane`; histórico, ANSI e join não são reconstruídos pela grade. A condição em `:398` exige igualdade literal antes de usar a grade na análise. Esse limite é correto, não uma pendência disfarçada.
- `crates/hangar-server/src/terminal_control.rs:222`, `:408`, `:433`, `:449`: canais limitados, leases, liberação do último consumidor e encerramento do filho estão implementados. Limites de linha/frame e de bytes processados também limitam sequências ANSI incompletas (`:10`, `:145`).
- `backend/app/plugin_bridge.py:421`: somente modo de controle explicitamente reconhecido deixa de contar como presença humana; erro, UTF-8 inválido e formato desconhecido continuam conservadores. Os casos adicionados começam em `backend/tests/test_plugin_bridge.py:520`.

## Problemas da rodada inicial — ambos corrigidos

### 1. Cursor válido na margem direita derruba o observador

**Local:** `crates/hangar-server/src/terminal_control.rs:305`.

`cursor_x >= columns` rejeita um estado válido do tmux: depois de escrever exatamente a largura da linha, sem newline e com wrap habilitado, o cursor fica em `cx == sx`, aguardando o próximo caractere. Exemplo sintético: pane com 20 colunas recebendo 20 caracteres ASCII sem quebra. A aquisição falha em `seed` ou uma captura posterior retorna `invalid terminal cursor` e encerra o actor, embora o pane esteja íntegro. Enquanto a tela permanecer assim, novas tentativas também falham e obrigam a reserva Python.

A implementação oficial do tmux permite `cx == sx` em [screen-write.c, movimento do cursor](https://github.com/tmux/tmux/blob/3.7/screen-write.c#L2443); [format.c](https://github.com/tmux/tmux/blob/3.7/format.c#L1680) expõe `base.cx` diretamente como `cursor_x`. Evidência por leitura do código, sem nova execução de tmux.

**Correção:** aceitar esse estado e representar o wrap pendente corretamente no checkpoint; não basta transformar o valor em uma coluna comum, pois o próximo caractere deve quebrar a linha. Acrescentar caso focado com largura exata, sem newline, e outro caractere depois. O teste de wrap existente (`crates/hangar-server/tests/terminal_control.rs:118`) não cobre essa borda.

### 2. Rejeição de resposta extra depende de como o pipe fragmenta os bytes

**Locais:** `crates/hangar-server/src/terminal_control.rs:79`, `:356`, `:364`.

O parser compara a identidade entre começo e fim do bloco, mas descarta essa identidade ao emitir `ControlEvent::Frame`. `frame()` aceita o primeiro bloco completo seguinte. Antes de escrever outro comando, `command()` verifica apenas `events` e `parser.frame`; não identifica resposta repetida/fora de sequência e nem considera um começo de marcador incompleto em `parser.pending`.

Cenário concreto: depois da resposta de `info`, a mesma leitura termina em `%begin 1` de um bloco extra. `events` fica vazio e `parser.frame` continua `None`. O próximo `capture-pane` é enviado; chegam o restante do cabeçalho e o corpo extra, que são aceitos como o texto da captura. Se os próximos blocos tiverem o formato esperado pelos checkpoints/metadados, a captura pode concluir com texto que não é a resposta daquele pedido. Um bloco ainda não lido do pipe tem o mesmo problema. Portanto, a rejeição implementada não é independente da fragmentação do transporte.

**Correção:** conservar a identidade e estabelecer correlação verificável dos comandos/respostas, rejeitando respostas duplicadas ou extras antes de publicar a captura. Cobrir o bloco extra fragmentado no cabeçalho, chegada posterior e identidades repetidas/fora de ordem. O teste `crates/hangar-server/tests/terminal_control.rs:276` envia os blocos extras juntos; prova apenas a rejeição quando já estão na fila ou com cabeçalho completo. Não executei uma nova reprodução; o cenário acima decorre dos estados do parser no diff.

## Limites da conferência

- Todos os oito arquivos previstos possuem alterações. Não há implementação da Task 3 neste intervalo; rotas internas, publicação, descarte de binding em voo e integração Windows/Python continuam fora deste parecer.
- `crates/hangar-server/src/terminal_control.rs:224` recusa Windows antes de criar processo. O teste específico está em `crates/hangar-server/tests/terminal_control.rs:328`, mas não há prova de execução Windows aqui.
- A liberação concorrente com uma nova aquisição pode rejeitar a aquisição que entrou atrás do último `Release` (`:449`), provocando uma tentativa posterior/reserva. Não encontrei publicação incorreta por esse caminho; não o classifico como bloqueio. Os testes de dois consumidores e troca de binding são sequenciais (`tests/terminal_control.rs:91`, `:284`), não provam todas as intercalações concorrentes.
- Cancelamento de pedido em andamento conserva a lease até expiração; descarte do pool fecha os canais, e timeout/EOF encerra o actor. A leitura do código não demonstra limpeza imediata no cancelamento, nem o contrato exige isso. As provas executadas informadas pelo implementador foram 16 testes Rust e 61 Python, além do tmux privado; nenhuma suíte ou teste foi repetido nesta revisão.
- O diff foi lido uma vez; trechos truncados pela ferramenta foram recuperados do mesmo pacote. Nenhum chamador inalterado foi aberto: os dois riscos encontrados estão completos no diff. A consulta externa ficou restrita ao comportamento oficial do cursor e ao protocolo de controle do tmux. Nenhum serviço, sessão do usuário ou arquivo de código foi alterado.

## Conferência das correções — `1f67623e..962fbe11`

- **Cursor na margem direita: consertado e conferido.** `crates/hangar-server/src/terminal_control.rs:322` aceita `cursor_x == columns`, mantendo a rejeição de dimensões nulas e coordenadas maiores. `restore_cursor` (`:178`) representa a coluna virtual como última célula física com `input_needs_wrap`; o checkpoint chama essa restauração em `:200`. O teste da grade (`crates/hangar-server/tests/terminal_control.rs:59`) comprova que o próximo caractere começa na próxima linha sem sobrescrever a margem. O teste tmux privado em `:345` espera explicitamente `pane_width=80` e `cursor_x=80` e exige captura idêntica à canônica.
- **Correlação de respostas: consertada e conferida.** `crates/hangar-server/src/terminal_control.rs:381` envolve cada consulta em marcadores próprios de início/fim. O consumo em `:393` exige o início correto, exatamente um bloco intermediário e o fim correto; `:397` confere números crescentes sem exigir contiguidade e flags iguais. O corpo só sai depois do marcador final. Assim, um bloco extra parcial em `pending` ou ainda no pipe deixa de poder substituir a resposta silenciosamente: falha na posição do marcador ou impede obter o fechamento esperado. A identidade continua disponível no evento do parser.
- **Evidência dos testes novos:** `crates/hangar-server/tests/terminal_control.rs:319` cobre resposta extra tardia com cabeçalho fragmentado; `:327` cobre fim ausente, incorreto e atrasado, incluindo a espera efetiva pelo fim. A fixture normal usa contadores com saltos, evitando a hipótese incorreta de ids consecutivos. O teste anterior de frames extras enfileirados foi preservado.
- **Sem nova quebra importante identificada no diff.** Os marcadores usam somente `display-message -p -l`, preservando a proibição de entrada, resize e alteração de ambiente. Timeout continua cobrindo a operação inteira com os três blocos; erro continua descartando o observador. O formato novo do evento do parser está refletido nos testes modificados. Pool, leases, alvos e integração Python não mudaram nesta rodada.
- **Verificação desta rodada:** leitura única do diff e da seção de correções do relatório. O implementador registrou reprodução vermelha dos dois problemas e 20/20 testes Rust aprovados após a correção; Python permaneceu inalterado com a evidência anterior de 61/61. Nenhum teste, serviço ou sessão foi executado nesta revisão. Permanecem os limites anteriores de integração da Task 3 e ausência de execução Windows; eles não são defeitos desta Task.

**Parecer final:** Task 2 aprovada, com os dois apontamentos importantes consertados e conferidos.


## task-3-report.md

# Task 3 — ponte terminal e reserva Python

Commit seletivo `a258733f`, branch `hangar-server-parte2c`, base `962fbe11`.
`git status --short` vazio após o commit. Sem push. Steps 1–4 marcados no plano e no combinado;
Steps 5–6 ficam para revisão independente do principal. Somente os 17 arquivos da Task 3 foram
commitados; controle da Task 2 e os formatos `side-events`/SSE não foram alterados.

## Resultado e interfaces

- `terminal_observer.py`: `configure(address, secret)` em memória; `lease(name, provider,
  binding_get)`, `capture(name, started)`, `frame_analysis(name, pane)` e
  `reduce(name, pane, memory, facts)`. Lease tem UUID por produtor. O contexto é explícito e
  não vaza nos yields do monitor. Duas fontes do mesmo provider/vínculo compartilham captura;
  fonte antiga não volta a eleger a conversa anterior.
- HTTP runtime usa `urllib` em thread, sem proxy, timeout 6 s (+0,5 s no await), corpo máximo
  16 MiB, UTF-8 estrito e JSON validado. Falha tem diagnóstico estático, uma vez por motivo em
  cada geração; não registra corpo, pane, segredo ou mensagem da exceção. Windows, provider
  excluído e ausência de configuração saem antes da resolução de alvo/HTTP.
- `shared_capture(name, max_age)` mantém assinatura/cache/inflight/shield e `max_age=0`. Os
  registros acompanham provider, binding, época local, geração da ponte e início da captura.
  `forget_frame` invalida metadados e resultados pendentes, sem cancelar a captura compartilhada.
  Frame antigo não entra no cache nem sai para o consumidor após `/clear`, troca de binding ou
  geração, inclusive com texto/nome/binding idênticos depois do reset.
- `StateMonitor(..., provider=None)` habilita Rust somente com provider Claude explícito;
  `ClaudeAdapter` passa `claude`. Cinco locais de memória permanecem separados no ramo Python
  (`prev_spinner`, `frozen`, `no_spinner`, `held_state`, `held_label`). Os seis fatos são lidos
  uma vez por tick. Resposta inteira válida substitui os locais; erro executa o bloco Python
  anterior com a mesma memória, pane e fatos. Permissão, Kimi, loop, shells, dedupe, publicação,
  fila, drain e consumidor SSE ficam no caminho anterior.
- `terminal_state::reduce_with_diagnostics` usa a mesma implementação de `reduce`; acrescenta
  `before_plugin` e `plugin_applied`, no ponto anterior à âncora plugin. A API `reduce` e seus
  goldens continuam iguais. Campos/labels inválidos do plugin recebem 400; a reserva preserva
  inclusive o `ValidationError` original de `StateEvent` para label `None`, em vez de convertê-lo
  silenciosamente em opção vazia válida.
- `PreviewBroker._loop` mantém lease por produtor e heartbeat independente, também quando só
  recebe sidecar. `None` cai no pane; vazio publica vazio com `md=True, full=True`. Análise Rust
  correspondente ao quadro fornece spinner/preview com `md=False, full=False`; ausência usa
  os extratores anteriores. `reset` preserva `_gen` e invalida metadados de captura.
- Codex `_state_stream` começa observação somente depois de `ensure_running`, para sessão
  terminal. Getter confere o mesmo dict de sessão, thread viva e ausência de headless. Heartbeat
  fica separado de `fila.get`, eventos nativos não esperam HTTP. Fechamento/cancelamento aguarda
  a task e a release, incluindo cancelamento durante o primeiro acquire. `state_monitor` fecha
  explicitamente a fonte interna para não adiar a release até o GC. Nenhum parser terminal
  substitui estado, pergunta ou prévia Codex; app-server/cano/envio não foram alterados.
- O gerador `gen_terminal.py` força `provider=None` e ponte desabilitada no contexto da referência.
  Teste executa o reducer Python com configuração Rust previamente ativa sem enviar HTTP.

## Porta privada, autenticação e protocolo

O mesmo processo Rust abre `127.0.0.1:0` para o roteador terminal, além do listener público
que mantém o bind original. `routes::serve_with_terminal_pool` permite pool explícito nos
ensaios; produção usa o construtor padrão. Ambos os listeners pertencem ao mesmo futuro e
fecham sob a parada existente. O público recusa `/__hangar_server/terminal` com 404; o privado
só monta esse endpoint. Saúde anuncia `terminal_address`, sem segredo.

O Supervisor limpa a ponte antes de cada geração, na saída e no `stop`, inclusive com
`proc is None`. Só configura após saúde com protocolo correto e processo ainda vivo;
endereço da saúde precisa ser IP literal loopback + porta não zero. Endereço ausente,
hostname, unspecified, externo ou inválido mantém a ponte desligada com diagnóstico.
A sonda pública continua mapeando `0.0.0.0`→`127.0.0.1` e `::`→`::1`.

Rota exige TCP loopback + comparação constante do segredo antes de consumir corpo. Qualquer
XFF externo/ inválido, inclusive cabeçalhos duplicados, recusa. Token do dono/convidado não
serve. Enum `acquire/capture/release/reduce` recusa campos desconhecidos; corpo tem teto de
16 MiB e leitura de até 6 s. Pedido inválido responde frase fixa com 400; erro do controle,
503. Release desconhecido não cria actor. Acquire inicial semeia; seguintes só renovam.
Capture mantém `binding/started/text/analysis` e usa os limites e alvo exato da Task 2.

`RUST_SERVER_PROTOCOL = INTERNAL_PROTOCOL = 2` no mesmo commit. Fake de saúde acompanha 2 e
mismatch testa explicitamente 1/campo ausente. `side-events` não mudou. Parte 2B deverá
reconciliar o número ao juntar seus novos eventos ao contrato.

## Falhas observadas antes da implementação

1. `(cd backend && uv run pytest tests/test_terminal_observer.py -q)`:
   11 falhas, módulo da ponte ausente. Novos casos de monitor/prévia/Supervisor produziram
   3 falhas esperadas antes dessas integrações.
2. `cargo test --manifest-path crates/Cargo.toml -p hangar-server --test terminal_routes`:
   E0599, `with_terminal_pool` ausente. O primeiro comando também encontrou `.json` no reqwest
   sem feature; o teste foi corrigido e o RED repetido ficou só na API ausente.
3. Casos focados adicionais, antes da respectiva correção:
   `semantic_capture_errors` falhou provider 503≠400; `specific_public_bind` falhou API de
   servidor privado ausente; 8 casos Supervisor falharam endereço anunciado/invalidado;
   `does_not_leak`, `retired_monitor`, `keeps_lease_alive`, `outer_codex_monitor`,
   `cancel_during_first_observer`, `forwarded_external_origin`, `error_is_visible` e
   `inconsistent_complete_response` reproduziram os respectivos problemas e ficaram verdes.
4. O teste adicional de label inválido inicialmente esperava um evento. O caminho Python
   original rejeita `None` em `StateEvent.options`; o teste passou a exigir esse erro original,
   preservando o contrato sem sanitização silenciosa.

## Conferência final e metodologia

Comando Python do conjunto autorizado:

```sh
(cd backend && uv run pytest tests/test_terminal_observer.py tests/test_shared_capture.py tests/test_state_classifier.py tests/test_preview_dedup.py tests/test_rust_server.py tests/test_internal_api.py tests/test_plugin_bridge.py tests/test_codex_adapter.py tests/test_sse.py -q)
```

Última rodada completa desse conjunto: **380 aprovados**. Depois dela, foi acrescentado somente
um teste de reserva para label inválido; o arquivo completo foi refeito com
`(cd backend && uv run pytest tests/test_terminal_observer.py -q)`: **37 aprovados**.
São **381 casos únicos conferidos**: 380 do conjunto + 1 caso novo; os outros 36 do arquivo
já estavam no conjunto. Contagens incluem parametrizações, sem somar reexecuções.

```sh
cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_terminal --test terminal_control --test terminal_routes
```

**28 aprovados**: 1 contrato golden + 20 controle + 7 rotas. Última rodada incluiu todas as
mudanças Rust. `git diff --check` passou. Nenhuma suíte inteira, formatter, instalador ou
serviço foi executado.

Os ensaios de rota sempre injetam `TerminalPool::with_program` com executável fake ou caminho
inexistente. Provaram um PID compartilhado entre dois produtores, acquire posterior sem novos
comandos, captura tipada e reap na última release. Outro ensaio bindou público em `127.0.0.2`
e privado em `127.0.0.1`, conferiu resposta privada 200 e fechamento de ambas as portas. A
ponte stdlib fez acquire/capture/reduce/release por HTTP real de loopback contra servidor
sintético mesmo com `HTTP_PROXY` configurado; servidor de teste não usa tmux. Os 20 testes de
controle existentes continuaram usando os sockets privados/execuções fake da Task 2.

## Limites e incidente de isolamento

Sem validação de app/serviço/backend vivo, sem restart ou instalação. Windows não foi
executado; permanece na captura/reducer Python. Não se afirma desempenho em produção.

Um RED da prévia estava sem dublê de `capture_pane` na reserva e tentou a leitura real
`tmux capture-pane -p -t %8 -S -200`, alvo fictício `%8`; voltou `can't find pane: %8`.
Nenhuma conversa foi lida e nenhum comando de escrita foi emitido, mas a tentativa contrariou
o isolamento pedido. O teste foi corrigido. Guarda autouse passou a bloquear `_run`/`RUN`
antes de I/O e a conferir no teardown se uma chamada bloqueada foi engolida. Testes novos
Codex têm a mesma guarda e patch explícito de alvo; reservas usam dublê de captura.
Não houve outra chamada real nos testes novos após a correção.

## Correções da revisão — rodada 1

Base congelada `a258733f`; correção seletiva `24ad5ea0`, branch `hangar-server-parte2c`.
Árvore limpa após o commit, sem push. Só `terminal_observer.py` e seu arquivo de testes mudaram.

Dois problemas conferidos:

- `BadStatusLine` e `IncompleteRead` pertencem a `http.client.HTTPException` e escapavam do
  tratamento anterior. `_request` agora captura essa família, registra só o motivo estático
  e devolve ausência para a reserva. Cancelamento continua propagado.
- O opener padrão seguia 301/302/303 e copiava `x-hangar-internal` para a nova requisição.
  `_NoRedirect`, derivado de `HTTPRedirectHandler`, recusa todo redirecionamento, inclusive no
  mesmo host. O `HTTPError` resultante segue para a reserva; nenhuma segunda URL é acessada.
  Limites de corpo/tempo, UTF-8, validação e geração continuam iguais.

RED inicial:

```sh
(cd backend && uv run pytest tests/test_terminal_observer.py -q -k 'http_protocol or refuses_every_redirect')
```

12 falhas e 4 aprovações. Seis redirecionamentos 301/302/303 (externo e mesmo host) tentaram a
segunda requisição no transporte sintético; 307/308 já eram recusados para POST pelo comportamento
anterior. Dois casos diretos levantaram `BadStatusLine`/`IncompleteRead`. Quatro testes de
integração tinham inicialmente um dublê chamado `http` que sombreava o módulo e levantava
`AttributeError`; isso foi corrigido no próprio teste.

RED repetido com o tratamento anterior de exceções e dublês corrigidos:

```sh
(cd backend && uv run pytest tests/test_terminal_observer.py -q -k http_protocol --tb=short)
```

6 falhas reais, todas `BadStatusLine`/`IncompleteRead`: pedido direto, captura/prévia e memória
Rust→Python. Após a correção, o primeiro comando acima aprovou os 16 casos novos.

GREEN final solicitado:

```sh
(cd backend && uv run pytest tests/test_terminal_observer.py -q)
```

**54 aprovados**. Metodologia: 37 casos anteriores + 17 novos (6 de protocolo, 10 combinações de
5 códigos de redirecionamento × 2 destinos, 1 de cancelamento), incluindo parametrizações,
sem somar reexecuções. Captura/prévia usam os dublês Python anteriores e a memória continua com
`frozen=2` antes da reserva confirmar idle no tick seguinte. Diagnóstico não contém o corpo
sintético ou o segredo dummy. `git diff --check` passou.

Novas reproduções usam exceções e handlers urllib totalmente sintéticos; a tentativa de uma
segunda localização falha antes de qualquer I/O. Somente segredo de teste foi usado. As guardas
`_run`/`RUN` permanecem ativas. O arquivo completo conserva o ensaio de loopback sintético
previamente autorizado; nenhum serviço do usuário, conexão externa ou tmux real foi acessado
nesta rodada. Não se repetiu o conjunto amplo nem testes Rust, pois esses caminhos não mudaram.

## Correção da revisão final — getter atual da prévia

Base congelada `24ad5ea0`; commit seletivo `d21a445b`, branch `hangar-server-parte2c`.
Árvore limpa após o commit, sem push. Só `preview.py` e o arquivo de testes da ponte mudaram.

A lease da prévia guardava a closure inicial de `stem_get`; `PreviewBroker.get()` podia trocar
esse atributo enquanto o produtor continuava vivo. Após `/clear` e registro do monitor da
conversa nova, a closure inicial ainda devolvia o vínculo anterior, aposentava o produtor e
parava a prévia com assinante ativo. A lease agora chama o atributo `self.stem_get` atual em
cada leitura, com `None` tratado. A regra de aposentar produtores realmente antigos permanece
igual; nenhuma seleção de provider ou outra lógica foi alterada.

RED antes da mudança de produção:

```sh
(cd backend && uv run pytest tests/test_terminal_observer.py -q -k follows_replaced_getter --tb=short)
```

**2 falhas**, ambas `TimeoutError` esperando `preview-b`: ponte ligada e desligada. A regressão
inicia o broker com closure A, recebe A, troca o getter pelo método `get()` no mesmo broker,
mantém a closure anterior em A, muda só a nova para B, registra a lease de monitor B e chama
`reset()`. Não recria o assinante.

GREEN após a correção:

```sh
(cd backend && uv run pytest tests/test_terminal_observer.py -q -k follows_replaced_getter)
(cd backend && uv run pytest tests/test_terminal_observer.py -q)
```

**2/2** na regressão e **56/56** no arquivo completo. Metodologia: 54 casos anteriores +
2 parametrizações novas (ponte ligada/desligada), sem somar reexecuções. O teste confere
`preview-b`, a mesma task ainda viva e exatamente um assinante após o reset. `git diff --check`
passou. Reproduções novas totalmente em memória, com `_request` e sidecar sintéticos, alvo
fake e guardas `_run`/`RUN` mantidas; nenhuma chamada nova a tmux ou rede real. Não se repetiram
suítes completas ou Rust, cujo código não mudou.

Uso no app/Windows reais e medição de carga em produção continuam fora dos limites aprovados
para esta entrega; não foram conferidos. A migração da Parte 2B pertence a outra tarefa, com
a reconciliação do protocolo já registrada. Esses limites não justificam ampliar esta correção.


## task-3-review.md

# Task 3 — revisão independente

**Conformidade final: aprovada em `24ad5ea0`.** O intervalo inicial `962fbe11..a258733f` implementa os arquivos e a integração pedidos; a rodada focada `a258733f..24ad5ea0` corrige os dois problemas encontrados no transporte.

**Qualidade final: aprovada.** R1 e R2 estão consertados e conferidos. Nenhum problema permanece aberto nesta revisão da Task 3. Uso real e Windows continuam com os limites registrados abaixo.

## Pontos bem implementados

- O monitor limita o contexto ao avanço do gerador, evitando repassá-lo ao consumidor, e mantém permissão, loop, shells e publicação no caminho existente (`backend/app/state.py:832`, `:1042`). A coleta única de fatos e a validação completa da resposta antecedem a troca da memória (`backend/app/state.py:893`, `backend/app/terminal_observer.py:246`). O teste de transição Rust→Python exercita a memória temporal (`backend/tests/test_terminal_observer.py:148`).
- Captura e análise usam vínculo, época, geração e início da leitura; o descarte antes/depois da captura e a associação com o cache evitam reaproveitar análise apenas por igualdade de texto (`backend/app/state.py:730`, `backend/app/terminal_observer.py:214`, `:236`). O teste de reset conserva deliberadamente vínculo e texto iguais (`backend/tests/test_terminal_observer.py:338`).
- A prévia conserva sidecar vazio e renova a conexão no produtor; Codex mantém eventos nativos e fecha explicitamente a fonte interna, com observação somente para sessões com terminal (`backend/app/preview.py:592`, `:627`, `:655`; `backend/app/adapters/codex/adapter.py:1289`, `:1499`, `:1530`).
- O listener privado é independente do bind público, dentro do mesmo processo. A rota autentica antes do corpo, verifica todos os cabeçalhos encaminhados, limita leitura e distingue erro de entrada de indisponibilidade (`crates/hangar-server/src/routes.rs:135`, `crates/hangar-server/src/terminal_routes.rs:60`, `:70`, `:92`). O Supervisor apaga a configuração antes da partida, depois da saída e mesmo em `stop` sem processo (`backend/app/rust_server.py:146`, `:198`, `:220`).
- A referência golden desliga a ponte, e a guarda dos novos testes registra também chamadas proibidas que uma reserva poderia engolir (`backend/tests/test_terminal_observer.py:24`, `:36`, `:399`). A correção do isolamento é verificável no código.

## Problemas importantes da rodada inicial — ambos corrigidos

### R1 — Erros de protocolo HTTP escapavam da reserva — consertado e conferido

**Local:** `backend/app/terminal_observer.py:85`.

`_request` captura `OSError`, `ValueError` e `TimeoutError`, mas a biblioteca HTTP também lança `http.client.HTTPException`, incluindo `BadStatusLine` e `IncompleteRead`. Essas classes não pertencem às exceções capturadas. Uma linha de status inválida ou resposta chunked truncada pode atravessar a ponte como exceção em vez de retornar ausência e executar a captura/reducer Python. Se acontecer no acquire inicial, o monitor nem entra no seu laço; no Codex, a task de observação pode terminar enquanto o estado nativo segue vivo.

**Prova isolada:** com `_http` substituído para lançar cada uma dessas duas exceções, `await _request(...)` propagou ambas. Resultado: `BadStatusLine ESCAPED BadStatusLine` e `IncompleteRead ESCAPED IncompleteRead`. Não houve socket, processo ou chamada tmux.

**Correção:** tratar a família `http.client.HTTPException` na fronteira do transporte com o mesmo diagnóstico estático e retorno de ausência. Acrescentar regressões focadas para protocolo inválido e resposta truncada, incluindo a continuidade da reserva do produtor.

### R2 — Redirect podia levar o segredo para endereço não validado — consertado e conferido

**Local:** `backend/app/terminal_observer.py:62`.

Desligar proxies não desliga redirects: `build_opener(ProxyHandler({}))` instala o `HTTPRedirectHandler` padrão. Em resposta 301/302/303 ao POST, ele monta outro pedido e preserva `x-hangar-internal`, inclusive quando o destino sai do loopback. Assim, uma resposta HTTP inesperada pode enviar o segredo a um endereço que nunca passou por `configure`, contrariando o limite de endereço interno validado. Este achado não pressupõe que o handler Rust normal emita redirects; identifica o comportamento incorreto da ponte ao receber uma resposta diferente do contrato.

**Prova isolada:** o handler da biblioteca instalada transformou o pedido com segredo fictício para `http://127.0.0.1:12345/__hangar_server/terminal` em pedido para `http://198.51.100.1/probe`, mantendo o cabeçalho. Resultado: `SECRET_FORWARDED True`. Apenas o objeto do pedido foi criado; nenhum endereço foi acessado.

**Correção:** desabilitar redirects nesse opener e tratar 3xx como falha da ponte. Acrescentar uma regressão com transporte sintético que prove que não ocorre segundo pedido e que a reserva recebe ausência.

## Conferência e limites

- Revisão do pacote do diff em uma passagem, dividida em blocos; o trecho truncado pela ferramenta foi recuperado. Consultas posteriores no próprio pacote serviram somente para obter números de linha.
- Consultas externas ao diff, dirigidas a riscos concretos: `tmux._pane_target` (`backend/app/tmux.py:206`), para conferir resolução de alvo; `pergunta_aberta` (`backend/app/askquestion.py:112`), para conferir o efeito da coleta antecipada de fatos; métodos do `TerminalPool` (`crates/hangar-server/src/terminal_control.rs:239`, `:269`, `:281`), para conferir renovação, troca de vínculo e liberação. O trecho de `reduce_with_diagnostics` em `crates/hangar-server/src/terminal_state.rs:360` foi consultado porque o corpo da precedência de perguntas estava cortado entre hunks. Também foi lido o método `HTTPRedirectHandler.redirect_request` da biblioteca padrão instalada, para o risco R2.
- A suíte não foi repetida. Evidência recebida: 381 casos Python únicos — 380 do conjunto focado mais um caso novo, sem somar reexecuções — e 28 Rust, provenientes do relatório da implementação. A única execução adicional foi a prova sintética dos dois problemas acima.
- Uso real no app/backend vivo e execução em Windows continuam sem conferência, conforme o escopo autorizado. Não houve serviços, instalação, push, mutação de código, índice ou HEAD nesta revisão; somente este relatório foi criado.

## Rodada focada final — `a258733f..24ad5ea0`

- **R1 consertado e conferido:** `backend/app/terminal_observer.py:92` captura `HTTPException` junto às falhas já tratadas, devolve ausência e conserva o diagnóstico estático. As regressões exercitam as duas exceções no pedido direto (`backend/tests/test_terminal_observer.py:554`), na captura/prévia (`:568`) e na transição de memória do reducer Rust para Python (`:600`). O cancelamento permanece fora desse tratamento e possui prova própria (`:665`).
- **R2 consertado e conferido:** `_NoRedirect.redirect_request` recusa a criação do pedido redirecionado (`backend/app/terminal_observer.py:62`), e o opener efetivamente usado por `_http` instala esse handler (`:69`). O teste usa o opener real com apenas o transporte HTTP substituído, cobre 301/302/303/307/308 tanto para destino externo quanto para o mesmo host e exige exatamente um pedido (`backend/tests/test_terminal_observer.py:640`). Assim, a regressão confere a montagem real da cadeia de handlers e a ausência do segundo envio.
- **Novas quebras:** nenhuma identificada nos dois arquivos alterados. Proxy desabilitado, limites de corpo/tempo, validação da resposta e descarte por geração permanecem intactos; `CancelledError` continua propagado. Os testes novos conservam os bloqueios de I/O tmux e usam somente dublês para as falhas reproduzidas.
- **Evidência recebida:** 54 casos aprovados no arquivo focado — 37 anteriores mais 17 novos: 6 de protocolo, 10 combinações de redirect e 1 de cancelamento. O relatório registra as falhas anteriores correspondentes, incluindo a correção do dublê que inicialmente sombreava o módulo `http`. Não executei testes nem novas provas nesta rodada; conferi o diff congelado e a seção final do relatório. Não há afirmação de uso real ou desempenho.


## final-review.md

# Revisão final independente — Parte 2C

Intervalo congelado: `a80a9e45..24ad5ea0` (`24ad5ea08e7c9c98c5719cb62e5ad4b970d3651c`). Revisão somente leitura de código/índice/branch; este relatório é o único arquivo escrito. Sem subagentes, suíte adicional, serviço, instalação, push ou tmux real.

## Pontos fortes

- O controle separa notificações de corpos, conserva identidade dos frames e só entrega a consulta depois dos marcadores exclusivos de início/fim. Captura canônica permanece a referência de histórico/wrap; a grade não responde ao PTY.
- Alvo exato, limites, expiração, encerramento do último consumidor e reserva Python têm implementações explícitas e provas focadas. O filtro de presença continua conservador para clientes desconhecidos.
- A rota privada autentica antes do corpo e fica fora do roteador público. Supervisor valida o endereço literal de loopback após protocolo; HTTP não usa proxy nem segue redirecionamentos; erros não registram pane ou segredo.
- A reserva do reducer reutiliza memória e fatos, a análise da prévia é vinculada ao quadro e o Codex mantém estado/prévia nativos. As fixtures com referência Python independente cobrem regras Unicode e sequências temporais, sem tornar Rust sua própria referência.

## Problemas

### Críticos

Nenhum identificado.

### Importantes

1. **A prévia pode parar definitivamente depois de `/clear` quando outra conexão SSE assumiu o broker.**
   - Local: `backend/app/preview.py:595`; consequência em `backend/app/preview.py:613`.
   - `PreviewBroker.get()` substitui `b.stem_get` quando uma conexão nova chega (linha 573), porque a closure anterior pode pertencer a uma conexão já encerrada. A nova lease guarda o objeto `self.stem_get` existente na entrada do loop, em vez de consultar o atributo atual. Portanto, ela continua lendo o vínculo antigo depois dessa substituição.
   - Caso concreto: celular e desktop compartilham o broker; a conexão que iniciou o loop fecha; a restante troca o transcript por `/clear`. O novo monitor registra o vínculo novo, mas a lease da prévia ainda retorna o antigo. `retired()` fica verdadeiro, `_observe_loop()` retorna e `_task` termina normalmente, mesmo com assinantes ativos. `subscribe()` só cria task quando `_task is None`, de modo que a prévia fica vazia/congelada até todos os assinantes saírem. Essa lógica de vínculo também existe com a ponte desabilitada.
   - Reprodução adicional inteiramente em memória: `_request` substituído por função assíncrona que retorna `{}`; `_pane_target` substituído por alvo sintético; `_run` e `RUN` bloqueados antes de I/O; sidecar devolve texto sintético pelo stem. Iniciar broker com getter A, consumir a primeira prévia, substituir o getter via `PreviewBroker.get()`, mudar somente o getter novo para B, iniciar lease de monitor B e chamar `broker.reset()`. Após um poll, resultado observado: `current_getter='b', broker_task_done=True, active_subscribers=1, preview_text='', task_exception=None`.
   - Correção: passar à lease um getter que consulte `self.stem_get` a cada chamada, por exemplo `lambda: self.stem_get() if self.stem_get is not None else None`, preservando a rejeição de produtores realmente antigos. Acrescentar regressão que troca o getter pelo método `get()` com broker ativo, mantém a closure anterior em A, registra monitor B e comprova nova prévia B sem reiniciar os assinantes. Cobrir ponte habilitada e desabilitada evita deixar a regressão no caminho Python.

### Menores

Nenhum identificado que justifique ampliar o diff.

## Evidência e limites

Foram lidos spec/plano, template de revisão, código alterado e contratos de chamada diretamente necessários em `sse.py`/`tmux.py`, testes, gerador e relatórios das três Tasks. As tabelas extensas do golden não foram reprocessadas nem reproduzidas no relatório. O HEAD conferido permaneceu congelado e o status versionado estava limpo.

Resultados focados anteriores, conforme relatórios: 398 casos Python únicos (381 já conferidos mais 17 novos da correção HTTP; reexecuções não somadas), 28 testes Rust (1 contrato com 58 entradas estáticas e 16 sequências/65 quadros; 20 controle; 7 rotas). Não repeti esses conjuntos. A única execução adicional desta revisão foi a reprodução sintética do problema acima, sem rede, tmux ou serviço.

## Comportamentos considerados e deixados fora do julgamento

- Uso no app/backend vivo e Windows real: provas não autorizadas nesta revisão; Windows continua no caminho Python. Não reivindico validação desses ambientes.
- Desempenho/carga em produção: sem medição autorizada; nenhuma afirmação de ganho é necessária para aceitar este contrato funcional.
- Migração da Parte 2B, outros providers e transporte de sessões sem terminal: fora da Parte 2C; conferi apenas que os pontos alterados preservam a seleção existente e o caminho nativo Codex. A reconciliação do protocolo 2B permanece registrada.

O executor deve decidir explicitamente sobre esses limites; eles não representam aprovação implícita de comportamento não conferido.

## Avaliação original em `24ad5ea0`

**Pronta para encerrar a entrega sem push: não, requer a correção importante acima.** A implementação atende os demais contratos examinados, mas a prévia deixa de funcionar em uma sequência normal de múltiplas conexões e `/clear`. Corrigir o getter e provar esse caso focado é suficiente para uma nova revisão limitada ao ajuste; não há motivo concreto para repetir suítes completas ou operar serviços vivos.

## Conferência da correção final — `24ad5ea0..d21a445b`

**Problema importante 1: consertado e conferido.** O HEAD desta rodada é `d21a445b9b9664d4678699b9ebf6f20a03da659f`; o status versionado conferido está limpo. O pacote contém somente a linha do getter em `preview.py` e a regressão parametrizada em `test_terminal_observer.py`.

A lambda da lease passa a consultar o atributo `self.stem_get` em cada leitura. Assim, a substituição feita por `PreviewBroker.get()` alcança a identidade do produtor existente, sem alterar a rejeição de fontes realmente antigas. O caso `None` continua explícito. A seleção de provider permanece `claude` ou `None`, e a renovação continua exclusiva de Claude: a alteração não habilita observação Rust para Pi/Kimi/omp nem modifica a seleção de fonte nativa Codex/sem terminal.

A regressão representa precisamente o caso descoberto: preserva a closure antiga em A, troca pelo método público `get()`, avança somente a closure nova para B, registra monitor B e executa reset. O assinante existente recebe `preview-b`; o teste exige a mesma task viva e um assinante, com ponte ligada e desligada. O relatório registra duas falhas por timeout antes da correção, dois casos aprovados após ela e 56/56 no arquivo completo. Não repeti testes ou probes nesta rodada; a revisão foi do diff e da evidência focada.

Nenhum problema crítico, importante ou menor residual identificado no ajuste. A contagem acumulada de evidência Python passa a 400 casos únicos (398 anteriores + 2 parametrizações novas; reexecuções excluídas); os 28 testes Rust anteriores continuam pertinentes porque o Rust não mudou.

Os limites listados acima foram julgados pelo executor: app/Windows reais e carga de produção não foram validados e não são reivindicados; Parte 2B e demais providers permanecem fora da tarefa, com reconciliação de protocolo registrada. Não há decisão pendente sobre esses limites.

**Pronta para encerrar a entrega sem push: sim.** A única falha importante da revisão completa foi corrigida na causa e coberta nos dois modos da ponte; a correção não amplia o escopo nem introduz mudança de seleção de provider.


## Registro da execução

Todas as Tasks e correções foram aprovadas por revisores independentes. O registro de decisões está na [conferência da entrega](2026-10-03-hangar-server-parte2c-verification.md#decisões-registradas). O plano guarda todos os Steps concluídos. Os relatórios acima preservam as reproduções, os comandos, as interfaces e os limites.

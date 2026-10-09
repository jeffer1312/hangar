# Harnesses — Claude, Codex, Pi, omp, Kimi

Decisões medidas, com data e número. As regras vigentes ficam na seção abaixo (o `CLAUDE.md`
só aponta para cá); a medição que sustenta cada uma mora na entrada de mesmo assunto.

## Regras vigentes

- **Retomada Codex pelo WebSocket local recebe o histórico inteiro, sem teto de 8 MB.**
  A fila reserva a entrada antes do envio; perda de resposta ou cancelamento conserva a
  tentativa como incerta. Só recusa explícita ou ausência de escrita permite reenviar
  automaticamente. Estar idle não prova ausência de entrega; o transcript confirma a entrada.
  Medição: [retomada longa e envio sem confirmação](#codex-retomada-longa-e-envio-sem-confirmação).

- **Observação terminal tem uma captura canônica por rodada, sem grade auxiliar.** O controle
  tmux confere sessão/pane a cada leitura; a análise acompanha esse quadro. Em Claude com
  terminal e o Rust de pé, o estado temporal é do `Monitor` do Rust, que pega o quadro do pool em
  processo; nos provedores que o Python observa (Pi, omp, Kimi), permanece no Python, sem outro
  HTTP. O cliente da ponte do observador usa somente HTTP sem proxy/redirect
  e não carrega certificados por pedido (a ponte ficou sem consumidor desde a parte 4; vale se
  voltar a ser usada). Medição:
  [custo da observação terminal](#custo-da-observação-terminal).

- **Deltas Claude/Codex acumulam antes de publicar.** Prévia, pensamento e input em voo têm
  buffer por sessão/geração: primeiro imediato, intermediários em 150 ms e último por timer/flush.
  Limpeza autoritativa cancela timers, aguarda publicação em voo e reconfere a sessão antes de
  limpar a fonte. Estado, permissões e fila continuam imediatos. Snapshot completo custa seu
  tamanho; não reconstruir ou parsear por delta. Medição:
  [coalescimento dos deltas](#deltas-claudecodex-acumular-antes-de-publicar).

- **Provedor omitido usa a última abertura humana do servidor.** A memória só muda após a
  criação confirmada; convidado e criação automatizada não a escrevem. Sem preferência válida,
  conta conectada vence CLI apenas instalado. A seleção inicial respeita a escolha feita à
  mão enquanto carrega; conta Codex padrão deslogada cede a uma adicional conectada.
  Ver [provedor padrão da abertura](#provedor-padrão-da-abertura).

- **O plugin do Hangar entra só por `--plugin-dir`, nunca pela pasta de skills.** Na cadeia
  de hooks, o primeiro plugin carregado fica por fora: `--plugin-dir` vem antes do marketplace,
  e a pasta de skills vem depois. A faixa dos mods (`plugins/hangar/hooks/ui.ts`) só recebe por
  `next(e)` o que os plugins de dentro desenham, e um mod que responde a faixa sem chamar
  `next` esconde tudo dos que estão por dentro dele. Os outros mods do repositório
  (`plugins/<nome>/` com `.claude-plugin/plugin.json`) entram pelo mesmo caminho, um
  `--plugin-dir` cada, sempre DEPOIS de `plugins/hangar`, sem pergunta do CLI e sem marketplace.
  O wrapper do `claude` no shell passa a mesma lista, lida de `~/.hangar/plugin-dir` (uma pasta
  por linha, a do Hangar primeiro; o arquivo antigo de uma linha continua valendo), que o
  backend grava só quando o CLI aceita a flag; `claude` cru (`command claude`) fica sem o plugin. O
  link antigo em `~/.claude/skills/hangar` sai no instalador: o mesmo nome nos dois lugares carrega
  um só, mas deixa erro em todo `/plugin` (medido em B, abaixo). Ver
  [faixa dos mods](#faixa-dos-mods-ordem-na-cadeia-medida-03102026).

- **Faixa e painéis dos mods saem no SSE por fonte própria, nunca na carona do `state`.** O
  `state` só sai quando a chave muda; o mod que relê com a sessão parada ficava velho no app. O
  plugin reenvia a faixa quando a ponte aparece e quando o `/pull` responde `faixa: false` (backend
  reiniciado começa sem ela): sem isso, faixa que não muda não voltava ao app. Ver [painel e clique](#mods-painel-clique-e-o-que-acontece-no-aparelho-04102026).

- **Aviso de mod (`$.ui.toast`) vai ao app por evento próprio (`plugin_toast`), com o prazo do
  mod.** O terminal o desenha por alguns segundos e ele não entra no transcript nem muda o estado.
  O backend guarda cada aviso até vencer e cada conexão do SSE recebe os ainda vivos com o tempo
  que resta; o app descarta pelo id o que já mostrou, porque a reconexão os repõe. Texto e nome
  longos são cortados, nunca recusados. Convidado (link compartilhado ou login próprio) não
  recebe aviso de mod. Cada app mostra no máximo 4 por vez e corta o texto longo na tela. Ver [aviso de mod](#aviso-de-mod-medido-04102026).

- **Botão de mod clicado no app é clique de mouse SGR no pane, achado pelo rótulo e confirmado
  pelo `ui.press`.** Nenhuma API do engine dispara o botão de outro plugin. Sem mouse ligado, com
  o pane em copy-mode, rótulo ausente ou repetido na região do site, o backend recusa em vez de
  clicar às cegas. O mouse é lido por `#{mouse_sgr_flag}` no tmux e por `#{alternate_on}` no
  psmux, que não tem as flags de mouse. A âncora da faixa (primeiro texto) conta o `label` de
  botão: sem isso, faixa que começa por botão deixava a linha dele fora da região.

- **Sessão Claude com terminal nasce com `CLAUDE_CODE_NO_FLICKER=1`.** O Claude Code só liga o
  mouse em tela cheia, e no Windows por SSH desliga a tela cheia sozinho ("fullscreen disabled:
  Windows over SSH"); a variável força a tela cheia mesmo sem `"tui": "fullscreen"` nas
  configurações. Sem ela, o clique dos botões de mod pelo app não tem onde chegar.

- **A ponte do plugin mantém seu loopback na porta pública quando o Rust está ativo.** Bind
  específico, inclusive na interface Tailscale, ganha uma entrada local só para `/api/plugin`;
  wildcard e loopback já atendidos não ganham outro socket. A cobertura IPv6 é conferida no
  socket, sem presumir dual stack. Nome MagicDNS, HTTPS e publicação do Tailscale não mudam.
  Sem Rust, o bind específico mantém o limite da reserva Python. Ver
  [ponte local com bind específico](#ponte-local-com-bind-específico-issue-80).

- **Clique do app que copia ou abre URL acontece no aparelho de quem clicou.** O plugin responde
  no lugar do `ui.copy` e do `process.run` de abridor de URL só quando a chamada vem do mod dono
  do botão, até 1,5 s depois de um press que o backend confirmou como vindo do app. Cópia e
  abertura levam o id da tentativa; chegando depois da resposta ao app, o backend recusa e o
  plugin deixa acontecer no terminal, para não sumir nem cair no clique seguinte.

- **A prévia corta cada linha na largura da conversa quando há painel ancorado.** A largura é o
  `bodyColumns` da faixa + 5; sem o corte, a borda `│` do painel vira texto da prévia.

- **Na prévia do pane, bloco cujo parágrafo termina colado no `⎿` é chamada de ferramenta, não
  prosa.** Vale com o `●` aceso ou apagado, no Python (`preview.py`) e no Rust
  (`terminal_state.rs`). Medição em [prévia da chamada em voo](#prévia-da-chamada-em-voo-o--pisca).

- **tok/s "agora" é medido no stream da resposta, nunca no transcript.** Do `message_start`
  ao fim da resposta, com o `output_tokens` real: sem terminal pelo `stream_event`; com
  terminal pelo `turn.step` do plugin (`rate.ts` → `POST /api/plugin/rate`). Nunca a partir
  do primeiro texto: o pensamento resumido chega segundos depois de gerado. Só o loop
  principal entra. Medida de outro transcript ou mais velha que a última resposta do
  transcript não vale; aí a reserva sai do jsonl e leva "~", porque inclui a espera pelo
  primeiro token. Ver [velocidade de geração](#velocidade-de-geração-tok-s).
  No Codex, "agora" e "últimas 10" usam essa reserva, com os deltas do uso oficial e os
  intervalos atribuídos ao modelo, sem o tempo de ferramentas. "1ª resposta" usa a média
  dos turnos observados até o primeiro delta de texto; sem essa medida, mantém a reserva.

- **Modo de abertura omitido herda a preferência do servidor.** `headless_default` nasce
  ligado para Claude/Codex; a escolha humana do dono na criação passa a ser o padrão.
  `headless=false`/`--terminal` e `headless=true`/`--headless` explícitos prevalecem.
  Chamadas automatizadas não gravam preferência. Convidado pode escolher para sua sessão,
  mas não acessa a configuração global. Providers sem suporte e isolamento `read_only`
  usam terminal quando o modo é omitido; `read_only` com sem terminal explícito continua
  recusado. O wrapper interativo do Codex sempre solicita terminal.

- **Lista e chat do Codex usam o mesmo estado nativo quando a conexão está saudável e assinada.**
  O hook é alternativa para estado indisponível; um `working` antigo não vence a interrupção
  confirmada pelo app-server. O retrato só vale para a mesma thread do rollout.

- **Codex: `/compact` chama `thread/compact/start`, fora da fila de prompts.** A troca entre
  terminal e sem terminal preserva a thread e suas escolhas; só confirma quando o destino
  carregou a conversa. Assinar eventos com `thread/resume` não sobrescreve sandbox nem aprovação.

- **`omp` é um FORK do Pi** — mesmo JSONL, mesmas extensões, e as diferenças pequenas já custaram
  bugs calados (binário próprio, raiz `~/.omp/agent`, sem `--session-id`, outros nomes de evento,
  subagente no mesmo processo). Raiz do agente omp tem UMA resposta: `app/omp_dirs.agent_dir()`.
- **A lista de modelos NUNCA é constante.** Conta Anthropic lê o picker ao vivo (cache de 1h,
  porque ler dirige o terminal); sessão de motor usa `/v1/models` do provedor. `/model <id>`
  grava default global — reponha o valor anterior.
- **Antes de digitar no composer do Claude, ESVAZIE ele, e o rascunho do dono volta:** digitar por
  cima gruda as mensagens num Enter só e o reconcile reentrega. O escritor Rust guarda o rascunho
  com o Ctrl+S do próprio Claude (`› stashed`), que o devolve sozinho no envio; sem envio, devolve
  com outro Ctrl+S só sobre composer vazio, e com envio incerto não aperta nada. Guardado já
  ocupado adia: um segundo Ctrl+S jogaria fora o que está lá. Nunca redigitar o rascunho lido da
  tela. O Python de reserva ainda apaga com `C-u`. Ver
  [rascunho do dono](#rascunho-do-dono-no-guardado-do-claude-05102026).
- **Antes de digitar no composer do Pi, PERGUNTE a ele** (`getEditorText`): a tela não distingue
  aviso de extensão de rascunho da pessoa, e comparar duas capturas não resolve.
- **Statusline e prévia vêm de sidecar do agente, não do pane.** O pane corta na largura da
  janela. `""` é resposta ("nada em voo"), `None` é ausência. Sessão Pi já aberta só publica
  depois de `/reload`.
- **Estado da sessão Claude vem do registro nativo (`<config>/sessions/<pid>.json`) quando ele
  existe e o pid vive**; marcador de hook e pane são o fallback. `idle`/`busy`/`waiting` são o
  estado da TUI escrito por ela mesma; `waiting` inclui diálogo aberto (`/model`), que o pane
  rebaixa. Nunca escrever nesse arquivo.
- **Permissão segurada para o app não aparece no pane nem no registro nativo: lista e `Monitor`
  leem a pergunta segurada.** Com o app aberto, o hook `perm.ts` segura a permissão: a TUI fica em
  "running PreToolUse hooks" sem cartão e o registro segue `busy`. O `Monitor` a lê dos fatos
  empurrados (`question`); a lista, do `held` dos fatos da lista. Ver
  [cartão de permissão segurado](#cartão-de-permissão-segurado-pelo-hook).
- **O `wire.jsonl` do Kimi não é bem-comportado**: nem toda escrita é turno (`config.update` com
  a sessão parada), e o main fica mudo quando delega. Quem decide é a fronteira de turno, não o
  mtime. `tool.result` não tem `uuid` — id é `res:<toolCallId>`.
- **Integração nativa do Codex: o Codex converte, o backend decide quando, o lançador só avisa.**
  Dois gatilhos, e só: abertura de sessão Codex e o botão Reconciliar. A integração não grava
  confiança de hooks; quem confia nos pendentes é o lançador, na abertura (regra abaixo). Fonte
  inválida nunca significa remoção.
- **Triagem do Claude não atravessa a importação para o Codex.** `skill-suggester.py`,
  `jev-command-gate.py` e `jev-answer-check.py` são excluídos pelo nome exato do arquivo,
  inclusive em caminhos Windows. O manifesto anterior retira só entradas já importadas;
  hooks nativos e nomes desconhecidos permanecem. A política entra na assinatura da fonte
  para invalidar o cache da próxima reconciliação.
- **Codex sem terminal: o app-server é do CANO, em stdio, e o cano é do Rust.** Com o
  `hangar-server` de pé, o Rust sobe, religa e mata o `hangar-cano` da sessão e conduz a thread;
  o Python só calcula argv/env (`launch_env` da política) e grava o arquivo da sessão
  (`session.patch_meta {cano}` / `session.clear_cano {pid}`). O `env` (tokens, `CODEX_HOME`) é
  pedido ao Python a cada subida e nunca vai a disco nem a log. O cano que sai é religado por
  evento (teto de 3 subidas seguidas, espera 5/10/20 s), sem varrer. Reiniciar com turno rodando
  é permitido (destrava turno preso); trocar o sandbox com turno rodando recusa com
  `erro_permissao_ocupada` (409). `initialize` repetido responde "Already initialized" e é
  sucesso; thread sem turno não tem rollout e o `resume` a recusa — processo novo abre outra,
  cano vivo segue pronto na mesma (ela já está carregada nele). Subida recusada vira
  `codex_conversa_nao_abriu` com o motivo, nunca sessão ociosa calada. Só `on-request` e
  `never` existem (`untrusted` morreu); o sandbox vai no `-c` da subida e trocar de modo reabre o
  servidor ocioso. Todo pedido do servidor tem resposta. Têm tela ou resposta própria: cartões de
  permissão, URL como cartão de link, `requestUserInput`, formulário MCP como pergunta nativa e
  `currentTime/read`. Todo o resto (`item/tool/call`, `chatgptAuthTokens/refresh`,
  `attestation/generate`, v1 legado, desconhecido) recebe `-32601` + nota, nunca sucesso vazio;
  pedido de thread de subagente nunca é descartado. Um cliente por cano.
  Falha vira erro com código, nunca passagem ao Python; o código Python fica para o modo `python`.
  Codex com terminal segue no Python até a 5C.
- **Processo do cano é um módulo só (`runtime/process.rs`), Claude e Codex.** Subir espera o
  `listen` por 10 s; matar confere a identidade do pid (pid reaproveitado nunca é morto) e apaga
  `cano-<chave16>*` só na pasta da sessão, nunca numa derivada de caminho gravado no arquivo. A
  varredura de órfãos roda UMA vez na subida do Rust, sobre `~/.hangar/claude-headless` e
  `~/.hangar/codex-sessions`, com dono = HOME (`HANGAR_CANO_OWNER`); o `matar_orfaos` do Python
  só roda no modo `python`. Teste ou backend isolado que sobe o Rust usa dono único ou
  `CP_RUST_NO_ORPHAN_SWEEP=1`, senão mata os canos reais da máquina. Windows não varre (sem
  `/proc`); mata por `taskkill /T /F`, aceitando 0 e 128.
- **Protocolo do Codex no Rust é tipado e tolerante** (`crates/hangar-codex`): todo campo usado
  existe no recorte do schema da versão conferida (`schema/<versão>.json`, teste
  `schema_check`); campo novo é ignorado; formato inesperado num método conhecido: a
  notificação é ignorada, a resposta segue com o padrão, e as duas vão ao diário
  (`rust.codex_decode`); item ruim do histórico vira desconhecido sem derrubar o turno; outra
  major.minor no `initialize` vira
  `codex_versao_nao_conferida`. Atualizar a versão conferida: `scripts/conferir-codex-schema`.
- **A rota do terminal Claude só recebe sessão Claude.** `route_sync`, `run_admin` e
  `answer_sync` abrem `prepare_session(name, "claude")`, que suspende a escrita sem vínculo
  Claude nem pane. Rota que atende outros provedores filtra pelo provedor antes (`/answer`,
  `/select`); erro que ainda escapar dela sai com código, nunca 500. Ver
  [cartão do Codex sem terminal](#select-do-codex-sem-terminal-não-passa-pela-rota-do-terminal-claude).
- **Nada no Hangar desvia a conversa da sessão para um proxy.** O `ANTHROPIC_BASE_URL` e o
  `model_provider` do Codex são do motor e do provedor, e o Hangar não os aponta para mais nada.
  Ligar o Jev numa sessão é só a chave no ambiente, para o `hangar-preview objetivo`. Por que o
  `jev-gateway` saiu: [superado.md](superado.md#o-jev-gateway-como-caminho-da-conversa).
- **Codex novo nasce em Full Access com ou sem terminal.** No sem-terminal, ausência de
  `permission_mode` também significa `Full Access`; a escolha manual continua valendo quando existe.
- **Scripts dentro de sessão sem terminal se identificam pela `CP_SESSION_KEY`.** Claude procura em
  `~/.hangar/claude-headless/`, Codex em `~/.hangar/codex-sessions/`; tmux só identifica sessões com
  terminal. Rename e `/clear` preservam a chave; `HANGAR_CANO_KEY` cobre sessões Codex já abertas
  antes dessa identidade comum.
- **Contas Codex adicionais têm `CODEX_HOME` próprio**; a identidade é `credential_id=codex:<home>`,
  nunca a chave. Sem migração, rotação ou troca automática por cota.
- **Abrir Codex nunca espera a sincronização**, nem no modal nem no pane: o lançador dispara o
  preparo da conta adicional (ou a reconciliação da padrão) no backend e sobe a TUI na hora, com a
  config que a conta já tem; o resultado vale a partir da próxima sessão. Preparo em andamento →
  o lançador confia a pasta ele mesmo; falha ao pedir vira aviso no pane, nunca sessão fechada.
- **A memória do Claude só é vista pelo Codex com uma CONVERSA ao lado dela** — e não basta o
  `.jsonl` existir: sessão que abriu e nunca conversou é descartada igual. Copiar só a `memory/`
  faz a reconciliação terminar `ok` sem trazer nada. Como o critério do detector não é documentado,
  o que foi copiado é conferido contra o que ele reconheceu, e a diferença vira aviso.
- **A consolidação da memória é o único item que gasta cota**: por isso é opt-in, não roda abaixo de
  25% de cota (decide antes de gastar), só sobe com a TUI, e vale a partir da SEGUNDA sessão — o
  índice entra na abertura, então quem manda consolidar não vê o próprio resultado.
- **Instruções nativas do Codex entram por `AGENTS.override.md`** apontando para o `CLAUDE.md`.
  Com sincronização ativada, o Claude vence a cópia do Codex; divergência é guardada em backup.
  Desativada, a abertura não altera os aliases. Reconciliar manualmente autoriza a atualização.
  Onde existe `AGENTS.md` de verdade, ele deixa de ser lido.
- **A ponte de skills é a ÚNICA dona das pastas de ponte**, é stdlib-only, e só mexe em symlink
  cujo alvo está numa fonte conhecida. Config alheia é conferida, nunca editada.
- **Motor de modelo: `engines.py` é stdlib-only**, é `ANTHROPIC_AUTH_TOKEN` (nunca `_API_KEY`),
  o env entra por `execvpe` dentro do pane (nunca `tmux -e`, que expõe a chave no `cmdline`), e a
  janela é `CLAUDE_CODE_MAX_CONTEXT_TOKENS`.
- **Plugin (`plugins/hangar`) é o caminho principal quando responde e prova a entrega; o tmux é a reserva automática, por sessão, e nunca sai do código.** Fallback
  é por AUSÊNCIA de long-poll vivo; entrega só vale com prova (rascunho confirmado, Enter aceito,
  composer vazio). `classic.*` não chega a plugin de `--plugin-dir`, `$` não atravessa `import`, e
  é um módulo por plugin. Meça no SSE, não em linha de log.
- **Observador tmux somente de leitura não pode bloquear o envio ao pane.** Após a recusa exata
  `client is read-only`, antes de qualquer tecla, `send-keys` tenta uma vez com cliente vazio,
  mantendo o pane de destino. Cliente explícito, outras falhas e psmux não entram nessa recuperação.
  Ver [cliente somente de leitura](#cliente-tmux-somente-de-leitura-não-bloqueia-envios-ao-pane).
- **Pedido de permissão só fica com o plugin com alguém no app E ninguém no terminal**: `tool.check`
  roda antes do diálogo, e segurar esconde o pedido de quem olha o terminal. Na dúvida (tmux mudo,
  Windows), não segura. Com o rodapé em modo auto ou dontAsk também não: ali o `ask` vai ao
  classificador (ou é recusado), e segurar troca essa decisão por um cartão de aprovação no app.
- **A ponte do plugin só atende a conversa que o Hangar acompanha na sessão** (a mesma do chat e
  das teclas): outro `claude` na sessão recebe 409 e a entrega não passa por ele.
- **Steer no Claude é só pelo botão**: `ctrl+x ctrl+s` INTERROMPE o turno em curso. Colado num
  recado (`steer:true`), abortaria o trabalho da sessão que recebe — o automático é só do Kimi.
- **Modo de permissão troca COM a sessão trabalhando** — é tecla, não texto. O guard de "está
  trabalhando" existe para o `/model`, que é texto.
- **Depois do BTab, o rodapé é lido na hora, sem pausa fixa**, e o Shift+Tab do app mostra o modo
  pedido na hora e junta as teclas num pedido só. Medição na entrada "Modo de permissão troca COM
  a sessão trabalhando".
- **"Padrão" na tela de criação vira o modo da conta AINDA na criação**, e `bypassPermissions`
  quando a conta não define nenhum: campo nulo virava flag ausente, e a sessão nascia no que a
  máquina tivesse. **Em plano, sessão cuja base é bypass não pergunta por ferramenta** — só o
  `ExitPlanMode`, que é o cartão do plano. Quem nasce no plano grava a base.
- **Codex sem terminal: `thread/start` leva o modelo, o esforço não** — não existe campo pra ele
  ali. Sem um `thread/settings/update` depois, o nível escolhido some no `model_reasoning_effort`
  do `config.toml`.
- **Claude sem terminal: o `claude` é filho do CANO, nunca do backend.** O cano é o binário
  `hangar-cano` quando o `rust_bins.find_bin` o acha (`CP_RUST_CANO_BIN`, `crates/target/release`,
  `~/.hangar/bin`), com o `cano.py` (stdlib) de reserva; os dois falam o mesmo protocolo. Um por
  sessão, escuta em socket local, nasce no escopo transiente do systemd e sintetiza um
  snapshot do que está em aberto; o backend só reconecta. O adapter (que muda sempre) fica no
  backend. Leitura do socket com `limit=16 MB` e embrulhada — leitor pendurado é sessão presa.
  Órfão é cano sem sidecar, não cano de backend anterior.
- **Claude sem terminal estaciona depois de `_OCIOSA_S` (65 min) parado** e sem nada em aberto: o
  processo sai, o sidecar fica, o próximo prompt sobe com `--resume`. Quem encerra por dentro tira
  a sessão da memória (`_encerrar`) — saída nossa não marca `returncode`.
- **Claude sem terminal só relê MCP/hooks/settings quando o processo nasce**: não há `/mcp
  reconnect` em `-p`. Recarregar = `_encerrar` + `acordar` (sobe com `--resume`), só ociosa. O
  motivo vai no `state` (`recarregar_motivo`: a marca do `mcpServers` + `settings.json` gravada
  na subida mudou — nunca mtime do `.claude.json`, que o Claude Code reescreve a toda hora) e a
  tela só oferece o botão com motivo; no menu ele fica sempre.
- **Claude sem terminal que não sobe para em `_TETO_SUBIDAS`**, com espera sob a trava de spawn
  (todo gatilho de drain passa por ela). A mensagem fica `desistiu` e o problema na faixa; só ação
  do usuário (`acordar`) abre outra rodada. Sessão com processo morto não é entregável.
- **Pensamento no Claude sem terminal só com `--thinking-display summarized`**: com `-p` a CLI
  ignora `showThinkingSummaries`. Pensamento e ferramenta em voo têm fonte e evento SSE próprios,
  nunca a prévia da resposta.
- **Runtime por conta não vira atalho** (`telemetry/`, `feedback/`, `image-cache/`, caches por
  config dir) — senão cada reconciliação acha "deriva" e gaveta de novo.
- **O diálogo de confiança do Claude Code derruba três coisas**: a chave do pre-trust usa barra
  normal no Windows, `is_overlay` tem que ignorar as linhas em branco do fim do pane, e o Enter
  às cegas cai em "No, exit".
- **A confiança na pasta é por conta: quem põe a sessão numa conta marca a pasta nela.** A criação
  marca na conta em que a sessão nasce; a troca de conta marca na de destino antes de reabrir.
- **Loop runner**: `LOOP_DONE` só fecha com confirmação humana; guardrails são max_iters,
  branch≠main e kill-switch. Loop ativo suprime o chain.
- **Plugin e marketplace do Codex usam os comandos nativos do CLI.** Nome do plugin + origem
  confirmada identificam um alias (o mesmo pacote tem nome diferente em cada manifesto);
  associar só pelo nome deixa o plugin antigo executando. Hook de arquivo em subpasta preserva
  o caminho relativo inteiro — homônimo na raiz não o substitui.
- **Falha do CLI do Codex deixa diagnóstico privado**, nunca stdout/stderr cru no log do
  serviço: a saída pode transcrever tokens.
- **Quem segura a abertura de uma sessão Codex é a TUI parada num widget**, não a
  sincronização. Sem thread não há sidecar, e o app fica esperando para sempre — o cartão de
  seletor pré-thread existe para isso.
- **A abertura com terminal não pergunta nada que o Hangar já sabe responder** (decisão do
  dono): a TUI sobe com `check_for_update_on_startup=false`; o lançador atualiza o Codex do npm
  antes do app-server (versão publicada consultada no máximo uma vez por hora, a tela mostra
  "atualizando o Codex"), e confia nos hooks pendentes pelo app-server (`hooks/list` +
  `config/batchWrite` do `trusted_hash`), porque eles vêm da sincronização do próprio Hangar.
  Falha em qualquer dos dois só registra na tela e a abertura segue. O cartão pré-thread
  reconhece o rodapé curto (`enter continue · esc skip`) além do antigo.
- **Pergunta assíncrona do Codex chega como `agentMessage` com `delivery: "async"`**, não como
  pedido JSON-RPC. Cada pergunta é independente; o eco da resposta local não responde outra de
  título igual.
- **O aviso de espera vem de `model/safetyBuffering/updated`**, não de temporizador local — o
  turno continua trabalhando.
- **Turno do Codex que não fala com o provedor vira `problema` no estado**, não "trabalhando"
  calado: `error` com `willRetry` é `codex_sem_conexao`, turno `failed` é `headless_turno_erro`.
  Some quando a resposta chega ou outro turno começa. Sem terminal a faixa oferece Reiniciar, e
  a retomada cai no provedor nativo quando o da thread não existe mais. Medição:
  [provedor fora do ar](#codex-provedor-fora-do-ar-o-turno-nunca-fecha).
  O `codexErrorInfo` decide antes do `willRetry`: `usageLimitExceeded`/`rateLimitExceeded` é
  `codex_limite_uso` e `unauthorized` é `codex_sem_login`, no Python e no Rust. Turno `failed`
  sem `codexErrorInfo` não apaga essas duas causas, que vieram no `error` anterior.
- **Stop do Codex encerra os comandos que o turno interrompido rodava.** `turn/interrupt` fecha
  o turno e deixa o processo vivo; depois dele vai um `thread/backgroundTerminals/terminate` por
  `processId` de item `commandExecution` ainda aberto DAQUELE turno. Comando de turno anterior
  (servidor de dev deixado de propósito) não é tocado. O `processId` é id do Codex, não pid. Ver
  [Stop, queda e pensamento](#codex-stop-queda-no-meio-do-turno-e-pensamento-medido-07102026).
- **Pensamento do Codex só existe com `summary` no `turn/start`**: sem ele o rollout guarda o
  raciocínio cifrado e o resumo vazio. Sem terminal todo turno pede `"detailed"`; com terminal
  quem decide é a config da TUI (o pedido vale para os turnos seguintes da conversa). O delta vai
  ao canal `thinking` e o `summary_text` do rollout vira bolha `thinking`, nos dois parsers.
- **Turno cortado pela queda do app-server vira `codex_turno_cortado`.** A vida anterior estava
  trabalhando, a reconexão relê a conversa e o último turno voltou `interrupted`.
- **Matar por pid de sidecar confere a identidade antes.** O cano tem `--escuta` e
  `--log …/cano-*` no argv; o app-server tem `app-server` e o `endpoint` da sessão. Pid vivo que
  não bate é outro processo (número reaproveitado depois de reiniciar a máquina): não mata.
- **Hook de fim de turno que reabre o turno vira aviso (`notice` `hook_prompt`), nunca fala da
  pessoa.** No Codex é a mensagem de usuário `<hook_prompt …>`; no Claude, o anexo
  `hook_additional_context` de `Stop`. O de `UserPromptSubmit` fica fora: vem em todo prompt.
  Nenhum dos dois grava o nome do script — quem se identifica é o texto do próprio hook.
- **Skill invocada no Codex vira aviso (`notice` `skill_loaded` com `skill: {name, path, body}`),
  nunca fala da pessoa.** O Codex grava o SKILL.md inteiro como mensagem de usuário
  `<skill><name>…</name><path>…</path>…</skill>` logo depois do `/nome` digitado; a interface
  mostra uma linha recolhida e abre o corpo sob demanda. O Pi usa outro formato
  (`<skill name="…" location="…">`) e ainda não é tratado.
- **A preferência da barra do Claude Code não autoriza sobrescrever `statusLine`**: desligada,
  o instalador preserva o que está lá.
- **Contexto, cota e nome do modelo da sessão Claude não dependem da statusline.** Quem mantém a
  barra própria tem o contexto e o modelo lidos do transcript (`claude_context.py`, campos
  `context` e `model` da lista) e a cota da API de uso; a barra do Hangar, quando traz o número
  ou o nome, continua valendo.
- **Hook nosso nunca bloqueia prompt, e a falha dele não some calada.** Em `SessionStart` e
  `UserPromptSubmit` o sufixo é `|| echo "<aviso>"` (texto puro, ASCII): sai com 0 e o aviso
  entra no contexto do modelo. Nos demais eventos o stdout não chega a ninguém e fica
  `|| exit 0`. O aviso não pode conter token terminado em `.py` — é por ele que o instalador
  reconhece a própria entrada. **Prompt barrado por hook (de qualquer origem) vira bolha "não
  chegou" com o erro do hook**, seja recado ou fala da pessoa.
- **Erro ao LER de um cano encerra o loop de leitura; só erro de MENSAGEM segue adiante.** Um
  `StreamReader` guarda a exceção de transporte e a relevanta em toda leitura seguinte sem nunca
  suspender: `continue` ali vira laço quente que não cede o event loop nem aceita cancel, e cada
  relevantada empilha frames no MESMO traceback, então o `logger.exception` custa quadrático.
  Leitura e processamento vão em `try` separados.
- **Progresso de MCP no Claude sem terminal vem de arquivo, não do stream.** O MCP grava cada
  etapa em `~/.hangar/tool-progress/<tool_use_id>.jsonl` (`{"t", "message"}` por linha; o id chega
  no `_meta` da chamada como `claudecode/toolUseId`) e o cartão aberto lê por
  `GET /api/sessions/{name}/tool-progress/{id}` a cada 2 s enquanto roda. A saída parcial do Bash
  vem do `tasks/<id>.output` do Claude Code, achado pelo processo (`procinfo.saida_de_comando`). Ver
  [Progresso de MCP](#progresso-de-mcp-no-claude-sem-terminal).
- **Anexo do Claude que não vira bolha não passa por `json.loads` no `/history`.** O
  `merged_history` lê só o relógio dele (`transcript.silent_attachment_timestamp`). Ramo novo de
  anexo no `parse_obj` entra também em `_ATTACHMENT_EVENT_TYPES`, senão some calado. Ver
  [anexos no /history](#history-de-transcript-grande-anexos-sem-bolha).
- **A prova de entrega pela tela espera a TUI recém-aberta, não só a aquecida.** O prazo de
  `_entrou_no_composer`/`_submeteu` (`_SUBMIT_CHECK_PRAZO`) cobre a primeira mensagem logo após
  criar a sessão; o caminho que dá certo sai na primeira leitura, só a falha paga o prazo. Ver
  [primeira mensagem na TUI recém-aberta](#primeira-mensagem-na-tui-recém-aberta).
- **Provider `orq` não tem pane nem processo.** A linha `<gid>-orq` é montada do `orq.json` e a
  conversa é a linha do tempo da execução; entrada, fim, renomear, interromper e parear respondem
  409 `erro_sessao_orq` (`api._recusa_orq`), e os três clientes escondem compositor, terminal,
  parear e rodar, com o botão "Falar com o árbitro" no lugar. Ver
  [Provider `orq`](#provider-orq-a-linha-do-orquestrador-não-tem-pane).
- **Confirmação de entrega lê o transcript de forma incremental, e o transcript continua sendo a
  prova.** Uma checagem pendente por sessão (`_agendar_confirmacao`); com a sessão trabalhando o
  intervalo dobra até 120 s. `committed_user_lines` e `fila_interna_pendente` leem pelo mesmo
  índice por (arquivo, provider), que só processa o que foi acrescentado. O `UserPromptSubmit`
  NÃO confirma: dispara também para prompt que outro hook barra. Ver
  [confirmação de entrega sem reler o transcript](#confirmação-de-entrega-sem-reler-o-transcript).
  Exceção: comando que o Claude sem terminal responde sozinho (resposta com `local_command_source`,
  como `/btw isn't available in this environment`) não vira linha no transcript. A resposta local
  confirma a entrada pendente mais antiga com o texto EXATO do último `/comando` que o motor
  escreveu, no Rust e no Python; sem isso a bolha ficava "aguardando confirmação" para sempre. O
  `local_command_source` não serve de prova: medido em 07/10/2026 (claude 2.1.292), ele traz a
  saída embrulhada (`<local-command-stdout>…</local-command-stdout>`), não o comando.
- **App-server efêmero do Codex sobe com `-c features.plugins=false` quando não usa plugins.**
  Ver [temporários `git-*` no `.tmp` do Codex](#temporários-git--no-tmp-do-codex).
- **Cota e catálogo do Codex vão por HTTP primeiro, com o app-server efêmero de reserva.** A rota
  é a do próprio binário (`chatgpt.com/backend-api/wham/usage`, `/wham/rate-limit-reset-credits`,
  `/codex/models?client_version=<codex --version>`), com o token do `auth.json` da conta. Token
  vencido ou fora do arquivo, resposta que não é 200, formato estranho ou rede fora → app-server,
  com linha `info` no log. O Hangar nunca renova o token. 429 na cota NÃO cai no app-server: ele
  bateria no mesmo backend. Ver [Cota e catálogo do Codex por HTTP](#cota-e-catálogo-do-codex-por-http).
- **A mesma conversa noutra conta só move o transcript depois que o `claude` antigo SAIU; a partir
  de 95% de cota a tela pede confirmação com aviso, e a partir de 99% a conta não aceita.**
  `POST /api/sessions/{name}/conta` espera os pids do cano e filhos antes do `move_conversation`;
  sem saída, recusa e religa na origem. A lista e a recusa saem de `_account_targets`
  (`ACCOUNT_LOW_PCT`, `ACCOUNT_FULL_PCT`). Na sessão com terminal o pane que reabre é outra vida
  de terminal da MESMA sessão: o runtime herda a chave pela prova da vida nova
  (`reborn_binding`), e só conversa nova esvazia a fila. Ver
  [Continuar a mesma conversa noutra conta](#continuar-a-mesma-conversa-noutra-conta).

- **Só é incerta a entrada terminal que começou a escrever.** O ator Rust abre o despacho de
  entrada como `staged`, e o escritor grava `MarkWriting` antes do primeiro efeito que pode
  entregar (socket nativo, plugin, digitação, colagem). No `Recover` (Rust e Python, que roda
  primeiro no boot) a tentativa só com despacho `staged` volta à fila (`interrupted_before_write`)
  sem trava. Despacho comum (`dispatching`) continua virando incerto. Ver
  [Reinício no meio de um adiamento](#reinício-no-meio-de-um-adiamento).
- **Entrada terminal recusada sem escrita não espera calada.** O ator Rust conta a série de
  adiamentos do escritor com `stage` (fora `question_open`, `not_ready`, `overlay`,
  `input_unavailable`). A espera entre as tentativas dobra a partir do tique até 8 s, a série é da
  linha (linha que sai da fila não passa a espera adiante) e o composer lido vazio libera a
  tentativa na hora; o log ganha
  uma linha por série; passados 30 s, `view.input_stalled` leva o código, o Python mostra
  `problema=terminal_input_*` e grava um `terminal.input_stalled` no diário. A fila continua
  tentando. Ver [Entrada terminal parada sem aviso](#entrada-terminal-parada-sem-aviso).

- **Transferência Claude → Codex não está aceita só porque a importação persistiu.** A prova
  precisa conferir os itens enviados pelo CLI após retomada, incluindo resultados completos.
  Na captura stdio 0.159.3, manter `tool_output_token_limit` da sessão conservou 144.000
  caracteres após reinício. Essa captura simulada não comprova interface, modelo real ou
  restauração física. Ver [transferência em validação](#transferência-claude--codex-captura-nativa-em-validação).

- **Painel de agentes do Claude Code não é menu, e o escritor não digita com o foco nele.** Abaixo
  do composer, o `❯` na frente de `●`/`◯` é o foco no painel de agentes; com o foco no rodapé
  (painel ou pílula "Enter to view tasks") o texto digitado some e o `x` para um subagente. Antes
  de digitar, Rust e Python dão um Esc, que só devolve o foco ao composer, e adiam se ele não
  voltar (no Rust, no máximo dois por linha da fila). O foco conta só com `❯` na frente de `●`/`◯`
  ou as dicas do próprio rodapé. Interromper com o foco lá manda esse Esc antes do que interrompe.
  Ver [o /clear e o rodapé do Claude Code](#o-clear-e-o-rodapé-do-claude-code).

- **A trava do `/clear` só sobe se o Enter pode ter saído e sempre tem saída.** Aceito, ou incerto
  nas etapas do Enter (`submit`, `submit_proof`; no Python, `*.submeter`). Incerto antes do Enter
  é entrega incerta comum. Passado o prazo com a sessão parada e sem conversa nova (nem no vínculo
  nem transcript nascido depois do despacho começando por `<command-name>/clear`, prova que vale
  60 s e que, ilegível, segura a trava), a trava sai, a operação fica
  recusada (`clear_not_applied`, a reabertura não a ergue de novo), a fila segue e a vista avisa.
  O `/clear` nunca é reenviado sozinho. Com o Claude trabalhando ele espera na fila do Claude
  Code, e a trava espera o turno.

- **Comando de barra adiado é erro na tela, nunca 200.** Ele não tem linha na fila e não roda
  depois sozinho (`erro_comando_nao_executado`).

- **Depois do Enter de um comando de barra, só o mesmo comando parado no composer ganha outro
  Enter.** O comando que rodou pode ter trocado a conversa (`/clear`): no Python o Enter às cegas
  caía na conversa nova e a reserva acusava vínculo mudado. A limpeza da fila que segue o `/clear`
  só acontece com a trava erguida (Enter pode ter saído); vínculo já trocado nela é o esperado, e a
  troca do vínculo esvazia a fila.

- **Pergunta do plugin interrompida pelo app sai na hora.** O Esc fecha o diálogo e o hook morre
  sem `/ask-fim`, deixando o long-poll aberto até a janela fechar. Só a pergunta lida antes do Esc
  é marcada; hook que ainda pergunta 2 s depois sobreviveu e ela volta a contar. Sem long-poll por
  mais de 3 s (Esc digitado no terminal), a pergunta também deixa de contar.

- **A régua da caixa de digitar do Claude Code pode trazer o nome da sessão, e há uma regra só para
  ela.** Com `claude --name` ou `/rename`, a régua de cima vira `──── nome ─`. No Rust, estado,
  prévia, foco no rodapé e tela dos mods leem a régua por `terminal_state::is_rule`; não criar
  outra regex de régua. O Python (`state.py`, `preview.py`, `plugin_screen.py`) segue com a régua
  pura: é limite da reserva sem Rust e do Windows. Ver
  [régua com o nome da sessão](#régua-com-o-nome-da-sessão-06102026).

- **Voz no app nativo:** WebRTC do próprio nativo (o transporte websocket do realtime recusa login
  ChatGPT); envio só a partir de fala do usuário, com espera de 1,5 s cancelada se ele voltar a
  falar. Medição em "Voz no app nativo".
- **Voz: modo Planejar:** nada vai à sessão até o fim; `finish_plan`, `ask_session` e `set_mode` só
  saem de fala do usuário, `finish_plan` em dois passos, e o envio só sai após silêncio do
  microfone. A leitura fora do projeto fica liberada (mesmo acesso da sessão). Medição em "Voz: modo Planejar".
- **Voz: o organizador só grava na própria pasta:** `workspace-write` com cwd em `~/.hangar/voz/arquivos`
  (fixa, nunca apagada) e sem raízes extras; o código da sessão é lido pelo caminho completo que a nota de
  contexto leva. Modelo e esforço vêm do card da voz (padrão: modelo do config do Codex, esforço `low`).

## O /clear e o rodapé do Claude Code

Medido em 06/10/2026, Claude Code 2.1.291, Haiku, backend isolado (issues #84 e #85, item 16 da
coordenação da migração).

**Rodapé.** Com agentes em segundo plano, abaixo do composer aparece `● main` / `◯ <subagente>`.
`↓` (o "↓ to manage") leva o foco à pílula ("Enter to view tasks") e, de novo, ao painel
(`↑/↓ to select`, depois `❯ ◯ … Enter to view · x to stop`). Com o foco lá, `/clear` digitado
não aparece no composer; o `x` para o subagente. O `←` com shells rodando abre outro diálogo,
"Background this session?", que é menu de verdade. Um Esc com o foco no painel ou na pílula só
devolve o foco ao composer: com o Claude trabalhando o turno continuou (o contador seguiu).
Telas em `backend/tests/fixtures/pane_agents_*.txt`, no contrato Python/Rust.

**Trava presa (#84).** O `/clear` digitado no painel não chegava ao composer: `prove_input`
falhava, a limpeza não se provava e a entrega voltava `unknown` em `input_proof`, sem Enter.
`Accepted | Unknown` erguia a `clear_barrier`; como a conversa não mudava, toda mensagem seguinte
voltava 503 `runtime_clear_barrier`, e o Recover a reerguia depois de reiniciar.

**Prazo.** O transcript novo nasce em menos de 1 s depois do Enter do `/clear`, e começa pelo
registro `<command-name>/clear</command-name>`. Com o Claude trabalhando, o `/clear` digitado fica
na fila do Claude Code e só rodou 21 s depois, no fim do turno: por isso a saída da trava exige a
sessão parada. Prazo de 10 s.

**Item 16.** Interromper o AskUserQuestion pelo app (Esc) mata o hook do plugin sem `/ask-fim`,
e o long-poll dele ficou aberto até a janela de 25 s fechar: `pergunta_pendente` dizia "aberta"
por até 35 s depois do Esc. O `/clear` mandado nesse intervalo era adiado (`question_open`) e,
sem linha na fila, sumia com 200; a mensagem seguinte não chegou em 60 s. Nos modos Rust e
Python. Depois da correção: `/clear` aplicado na hora e a mensagem seguinte entregue uma vez.

Fica de fora: no modo Python um `/clear` bem-sucedido volta 400 "resultado terminal incerto"
(a troca da conversa acusa vínculo mudado depois do Enter), também antes desta correção.

## Cartão de permissão segurado pelo hook

(06/10/2026, parte 4 da migração, Task 6; Claude Code 2.1.291, Haiku, modo manual, backend
isolado.) Sintoma: depois de pedir um `Bash`, a lista ficava `working` enquanto o chat mostrava o
cartão. Reproduzido duas vezes: com a lista do dono aberta (`app_presente`), o hook `perm.ts`
segura o `tool.check` em long-poll (`/api/plugin/ask`, janela de 5 s) e a TUI fica em
"running PreToolUse hooks… 6/10" sem desenhar cartão nenhum; o registro nativo segue `busy` e o
marcador `working` durante toda a espera (45 s). O `Monitor` já via a pergunta pelos fatos
empurrados (`question` com `perm:`); a lista não tinha fonte: confiava no registro e não
capturava. O Python de reserva (`registry.list_with_state`) tem o mesmo defeito.

Conserto: o `POST /internal/list/facts` leva `held` (`pergunta_pendente` das linhas Claude com
terminal, contrato 32), e a lista a mostra como o `Monitor` (`terminal_state::held_question`:
"ferramenta: resumo", Yes/No). Sequência gravada em `gen_terminal.py`
(`permission_card_after_bash`); `contract_terminal::permission_card_after_bash` confere lista e
`Monitor` rodada a rodada.

Com o `Monitor` vivo, a lista lê o último `state` dele (`state/published.rs`, por nome e session
id, limpo quando o `Monitor` acaba ou publica `dead`) e não captura o pane da sessão: nem a
classificação, nem a statusline, nem o radar de limite. Com 5 chats de 20 sessões sem marcador,
13 → 9,75 capturas por segundo e o Rust de 39 para 30,5 ms/s parado
(`docs/migracao-rust/parte4/medicao.md`, Task 6).

Aberto: depois do Esc o hook é cancelado sem `/ask-fim`, e a pergunta segurada vale até vencer
(35 s depois do último poll); o chat (e agora a lista) mostra o cartão nesse intervalo.

## Prévia da chamada em voo: o ● pisca

Medido em 05/10/2026, Claude Code 2.1.289. Com um comando rodando, a TUI desenha a chamada como
`● <descrição que o modelo escreveu> · 2s` e, embaixo, `⎿  $ comando (3s)`. Ao terminar, o bloco
vira `Ran 1 shell command`. A primeira linha não começa por `Bash(` nem por um verbo da lista, e o
`●` dela pisca: em quadros alternados ele some e a linha fica só recuada.

A prévia elegia a descrição como prosa nos quadros com `●`. Nos quadros sem ele, a varredura caía
no bloco de prosa anterior, e a descrição grudava nele como continuação. A prévia trocava a cada
piscada, e a pele Terminal subia e descia o tempo todo. Uma sessão real com dois `sleep 6`
registrou 36 trocas de prévia; com a regra, 4 (vazio, as duas frases e o "Pronto.").

A regra olha a forma, não o vocabulário: o parágrafo da chamada termina colado no `⎿`, e o da
prosa termina em linha em branco ou no próximo `●`. Os dois quadros reais estão em
`backend/tests/fixtures/pane_ferramenta_em_voo_*.txt`, cobertos pelo teste da prévia e pelo
contrato Python/Rust. Efeito colateral aceito: um aviso do sistema seguido de `⎿` (o limite de
uso em `pane_limite_uso.txt`) também deixa de virar prévia, e ele nunca foi texto da resposta.

## Cliente tmux somente de leitura não bloqueia envios ao pane

Medido em 04/10/2026, Linux com tmux 3.7c. Envios ao Claude pelo celular falharam repetidamente
com `client is read-only`; a rota os apresentou como envio incompleto, e as teclas de limpeza
do composer também foram recusadas. Havia um observador de controle anexado com as flags
`read-only,ignore-size,no-output`.

O `-t` escolhe o pane, mas `send-keys` também infere um cliente. Quando o cliente escolhido é o
observador, o tmux recusa o comando antes de enviar qualquer tecla, como confirma o
[código do tmux 3.7c](https://github.com/tmux/tmux/blob/3.7c/cmd-send-keys.c#L159).
Essa recusa não significa que o texto foi digitado pela metade.

A recuperação em `tmux._run` se limita ao código 1 e à mensagem exata dessa recusa. Repete o
comando uma vez com `-c ""`, conservando alvo e teclas. O cliente vazio não resolve para um
cliente anexado, conforme a [resolução de clientes do tmux](https://github.com/tmux/tmux/blob/3.7c/cmd-find.c),
e a proteção do observador permanece. A recusa fica registrada como `mux.readonly_client` no
diário, sem texto enviado. Não há repetição para outras falhas, cliente explícito ou Windows.
O caminho de sucesso continua com uma chamada e não exige uma sondagem de versão.

Reprodução em servidor tmux isolado: com apenas o observador somente de leitura anexado, o
comando original voltou com a mesma recusa. Após a correção, texto e Enter chegaram a um
processo `cat`, que devolveu a linha submetida. O observador continuou somente de leitura,
e o servidor de teste foi encerrado. Os testes também cobrem retorno de erro após a única
tentativa de recuperação, envio literal de `-c`, cliente explícito e isolamento do psmux.

## Transferência Claude → Codex: captura nativa em validação

Medido em 03/10/2026 com o binário ELF bruto `codex-cli 0.159.3` no Linux, pelo roteiro
`scripts/probe-claude-to-codex.py`. O código está integrado na árvore isolada de implementação;
não foi ativado no serviço nem recebeu aceitação completa. Esta é a única versão conferida
para esta captura.

Método: fonte Claude artificial → `convert_snapshot` → `prepare_import` → app-server stdio
nativo → pedido Responses HTTP com resposta SSE simulada em `127.0.0.1`. `HOME`, `CODEX_HOME`,
workspace, fontes e captura ficam em diretórios privados descartáveis. Não há autenticação,
inferência real ou desvio de sessões existentes para o simulador. `gpt-5.6-luna` identifica o
modelo do catálogo usado na captura, não um padrão fixado pelo recurso.

| Conferência | Resultado da segunda execução |
| --- | --- |
| Fonte artificial | 159.496 bytes; ramo selecionado sem o fork rejeitado nem resumo substituto. |
| Itens importados nas duas capturas | 11 itens na mesma ordem, projeção de 158.374 bytes e SHA-256 `82b21b55e1325dcf7e17842e326ac07e1020e0e5ed774baf1089b88ecc8a9120`. |
| Resultado longo / argumentos | 144.000 caracteres completos e mesmos hashes após importação e após reinício do app-server. |
| Conteúdo anterior à compactação, instruções, arquivo e chamada sem resultado | Comparados com originais independentes; zero diferença de bytes em ambas as capturas. |
| Imagem PNG artificial | 69 bytes e SHA-256 idêntico; confere transporte, não compreensão da imagem. |
| Ferramentas ativas | `exec`, `functions`, `request_user_input`, `wait`; `Read`, `Edit`, `Bash` ficam no histórico. |
| Pedidos ao simulador | Zero antes do turno; dois turnos simulados concluídos, um em cada captura; zero pedidos de compactação. |
| Fonte maior | 559.496 bytes recusados com `session_transfer_context_budget_exceeded`, sem pedido HTTP; não havia Claude físico a restaurar. |

A projeção compara os campos históricos originais, omitindo IDs gerados pelo Codex e campos
adicionais do protocolo. O pedido HTTP inteiro não tem hash igual entre capturas: inclui
conteúdo nativo além da projeção. A fonte é maior que a projeção porque contém envelopes e
registros operacionais que não são itens de contexto. Os hashes dos textos anteriores à
compactação, instruções, arquivo, chamada sem resultado, argumentos, resultado longo e imagem
são conferidos separadamente; a referência não vem apenas da saída do conversor.

A primeira execução falhou ao confirmar a saída dos filhos do processo de preparação: o
fechamento normal foi consultado cedo demais. A correção incorporada em `cb23906e` espera
até 5 s por `wait_import_exit`, conferindo o dono do processo e preservando a causa original
quando o fechamento também falha. A segunda execução passou a captura; isso não prova
restauração da origem, comportamento de queda dura ou ausência de filhos em toda plataforma.

Os limites desta prova vêm do [catálogo oficial na revisão fixa
`01fc69f4026735edfdf6789820549727a4867b11`](https://raw.githubusercontent.com/openai/codex/01fc69f4026735edfdf6789820549727a4867b11/codex-rs/models-manager/models.json)
e do [padrão do protocolo da mesma revisão](https://github.com/openai/codex/blob/01fc69f4026735edfdf6789820549727a4867b11/codex-rs/protocol/src/openai_models.rs#L389):
janela configurada de 272.000, parcela utilizável de 258.400 (95%) e limiar de compactação
de 244.800 (90%). Esses valores não comprovam acesso ou capacidade atual de um servidor real;
o máximo anunciado pelo catálogo não foi usado como janela desta captura.

O orçamento soma bytes UTF-8 dos itens serializados, 10.000 por imagem, instruções/ferramentas
conhecidas e reserva de continuação; compara o total com o menor limite utilizável/de
compactação. É uma estimativa conservadora, não uma contagem de tokens pelo tokenizer, e pode
recusar conteúdo que caberia. Capacidade desconhecida ou mídia não suportada têm erros
próprios; não levam a resumo, compactação ou descarte para fazer caber. O limite de resultado
é por sessão e acompanha a importação/retomada, sem alterar a configuração global da conta.
Os zeros de uso da resposta simulada não medem tokens ou custo do primeiro turno.

Para repetir a captura, com autorização de execução e o ambiente Python já instalado, a
partir da raiz do checkout que contém a implementação:

```bash
uv run --project backend --no-sync python scripts/probe-claude-to-codex.py \
  --binary "<caminho-absoluto-do-binário-ELF-bruto-Codex-0.159.3>" \
  --output-dir /tmp/claude-to-codex/native-proof-nova
```

O diretório de saída precisa ser novo, diretamente sob `/tmp/claude-to-codex`; o roteiro não
reutiliza nem apaga uma captura. `results.json`, fonte e pedidos brutos permanecem privados,
fora do versionamento. O comando não instala dependências nem substitui a aceitação do produto.

Permanecem pendentes: conversa real integral e inferência com modelo/conta reais; interface e
foco; guardas com origem/processos reais; modo Plano e permissões efetivas; WebSocket/TUI
desta implementação; história/citações/mídia, fila e Arquivo no fluxo completo; recarga e
falhas físicas; Windows e tokens/custo reais. Pesquisa anterior de TUI/WebSocket não aceita
esta implementação. Testes automatizados, lint e typecheck não foram executados nesta etapa.

Limite de recuperação: a trava do backend não controla digitação direta na TUI da origem;
o estado é revalidado logo antes de pará-la. Queda dura sem dono ou captura final da árvore
de processos mantém `restore_failed`: a ausência do processo raiz não prova que todos os
filhos saíram, e não se mata um processo por semelhança. O erro e o histórico da origem
continuam disponíveis; `POST /api/sessions/{name}/recarregar` tenta recuperar, sem promessa de
recuperação automática em toda falha. Texto disponível de pensamento é histórico identificado;
assinatura não vira raciocínio Codex, e conteúdo redigido/criptografado não é recuperável.

## Continuar a mesma conversa noutra conta

Medido em 02/10/2026 numa sessão descartável sem terminal: com `parar` sem esperar a saída, o
`claude` que estava morrendo gravou `last-prompt`, `atis-latch` e `cost-state` pelo CAMINHO antigo
depois do `rename`, e recriou na conta de origem um `.jsonl` de 3 linhas com o mesmo id. Esse
arquivo faz o `conta_de` achar a conversa em duas contas. Esperando os pids, a ida e a volta
entre contas não deixaram nada na origem, e a conversa lembrou a palavra-chave dos turnos
anteriores (sem terminal e com terminal).

Os limites de 95% e 99% foram escolha do usuário: continuar reenvia o contexto inteiro no primeiro
turno, então a 99% a conta acaba nele; entre 95% e 98% ela ainda serve, mas quem escolhe precisa
confirmar sabendo disso. Contar a janela mais cheia, como o `sugerir_claude`.

Medido em 04/10/2026 (commit `584e65c5`, servidor Rust na frente), em duas sessões descartáveis
com terminal: `POST /conta` respondia 500 com e sem mensagem na conversa, e a conta trocava mesmo
assim. A troca mata o pane e abre outro dentro de uma ação só do `coordinator.change`; o vínculo
resolvido depois tinha outra impressão digital, e o `change` recusava a chave nova (`mudança de
modo não pode trocar a chave durável`) ou, com o agente ainda fora de vista, a espera dos
escritores estourava `BindingChanged (sem vida atual)`. O slot ficava com o vínculo velho, e a
lista e o SSE da sessão passavam a estourar `BindingChanged`. Com tmux e `claude` reais e o
coordenador isolado do backend, a mesma sequência falhou sem o conserto e, com ele, manteve a
chave e validou o vínculo novo. O `clear` da fila comparava o caminho do transcript, que muda com
a conta; passou a comparar a conversa.

## Confirmação de entrega sem reler o transcript

Medido em 30/09/2026 pela sessão `Projetos`: backend a 100% de um núcleo. py-spy de 20 s deu
1046 de 1303 amostras em `_confirm_and_drain`, relendo `.jsonl` de 3 a 24 MB duas ou três vezes
por chamada. Havia 25 `threading.Timer` vivos para 3 sessões com entrega sem confirmação:

- Send, fim de turno e a própria checagem agendavam cada um o seu Timer, e as cadeias se somavam.
- No ramo `working`, a entrada nunca vira `desistiu` (`confirm_only`), então a checagem se
  reagendava a cada 8,5 s pelo turno inteiro.

Correção:

- `_agendar_confirmacao` mantém uma checagem pendente por sessão. A que roda antes vence, e a mais
  tardia é trocada.
- Com a sessão trabalhando, o intervalo dobra até `_CONFIRM_WORKING_MAX` (120 s).
- O índice `_CommittedIndex`, que já existia só para o Codex, passou a servir Claude, Pi, omp e
  Kimi. Ele guarda o offset, as linhas confirmadas e a fila interna (`queue-operation`), e relê do
  início quando o arquivo troca, encolhe ou a âncora dos 256 bytes antes do offset não bate.

Confirmar pelo hook `UserPromptSubmit` (texto do prompt no payload, sem ler o transcript) foi
considerado e descartado. Os hooks do evento rodam em paralelo, e o nosso não sabe se outro barrou
o prompt: confirmar ali escondia a bolha "não chegou" do prompt barrado (regra "Prompt barrado por
hook"). O hook também roda antes de a linha existir no `.jsonl`, e a bolha da fila sumiria antes da
real. A recheca periódica do turno longo continua espaçada, não removida: mensagem orientada no
meio do turno entra como `attachment/queued_command`, sem `UserPromptSubmit`, e sem a recheca
voltaria a bolha fantasma dos `test_turno_longo_*`.

## Cota e catálogo do Codex por HTTP

Em 29/09/2026 (codex-cli 0.159.0) o binário traz as rotas do `backend-client`: com base
`https://chatgpt.com/backend-api` ele usa o estilo `/wham/...` (`/wham/usage`,
`/wham/rate-limit-reset-credits`); o catálogo sai de `https://chatgpt.com/backend-api/codex` +
`/models?client_version=`. Cabeçalhos: `Authorization: Bearer <access_token>`,
`ChatGPT-Account-Id: <tokens.account_id>`, `User-Agent: codex-cli`. A leitura antiga dizia que
o endpoint "não é público"; ele é o mesmo que o CLI chama, com a mesma credencial.

- **Mapeamento da cota**: `rate_limit.primary_window`/`secondary_window` →
  `usedPercent = used_percent`, `windowDurationMins = limit_window_seconds / 60`,
  `resetsAt = reset_at`. A lista de redefinições (validade, estado, título) só vem na segunda
  rota, com `expires_at` em ISO (vira epoch truncado, igual ao app-server). As duas saem em
  paralelo; sem redefinição disponível, falha da segunda não conta.
- **Paridade medida** nas duas contas desta máquina: janelas, percentuais, resets e redefinições
  idênticos ao `account/rateLimits/read`; catálogo idêntico ao `model/list` depois do `parse`
  (id, nome, descrição, esforços por modelo, esforço padrão; `visibility != "list"` =
  `hidden`; ordem por `priority`). `client_version` muda a lista: `0.151.0` devolve 6 modelos,
  `0.159.0` devolve 10, sem ele é 400 — por isso a versão sai do `codex --version` do mesmo
  binário (~10 ms).
- **Tempo** (mediana de 5, por conta): cota por HTTP 0,56–0,62 s contra 0,83–0,91 s do
  app-server; em série as duas rotas davam 0,9–1,0 s, igual ao app-server. O ganho maior é de
  recurso: o app-server gasta ~0,41 s de CPU e ~200 MB de pico por leitura de cota; o HTTP,
  ~0,02 s de CPU dentro do backend. Catálogo: HTTP 0,45 s (picos de 1,5–2 s) contra 0,22 s do
  app-server, que responde do `models_cache.json` local — mais lento em tempo de parede, mas sem
  processo (0,26 s de CPU a menos por leitura), e a lista tem cache de 10 min.
- **O que NÃO se faz**: renovar o token. O refresh é do CLI, e girar o refresh token por fora
  deslogaria o CLI. Token a menos de 60 s de vencer já vai pro app-server, que usa a credencial pelo
  próprio CLI.

## Temporários `git-*` no `.tmp` do Codex

Em 29/09/2026 (codex-cli 0.159.0) `~/.codex/.tmp` tinha 7.245 pastas `git-XXXXXX` vazias
(`HEAD` + `objects/` + `refs/`) e `~/.codex-<conta>/.tmp` outras 3.434. Na largada, todo
`codex app-server` roda `git ls-remote` em cada marketplace `source_type = "git"` usando um
diretório temporário; se o processo sai antes de a conferência acabar, o temporário fica. Os
comandos `codex plugin ... --json` não fazem isso. `codex_appserver.perguntar` (cota a cada
poucos minutos por credencial e catálogo de modelos) mata o processo ~1 s depois de subir, então
cada leitura deixava uma pasta. Medido em `CODEX_HOME` descartável com 3 marketplaces git:
3 largadas mortas = 3 sobras; fechando o stdin também 3 (sair "limpo" não resolve); com
`-c features.plugins=false`, 0. `account/rateLimits/read` e `model/list` respondem igual sem
plugins. O `CodexNativo` do importador precisa de plugins e segue deixando uma sobra ocasional
por rodada.

## Primeira mensagem na TUI recém-aberta

Em 28/09/2026 a "Nova conversa" do app nativo (cria a sessão e manda o texto em seguida) dava
"envio incompleto: o composer foi limpo e a mensagem NÃO foi enviada" toda vez, com a mensagem
chegando e o Claude trabalhando nela. Log: `etapa=linha.submeter` — o texto seguia no composer
1,0 s depois do Enter. Medido em sessões `cx-*` (Claude Code 2.1.283), tempo depois do rodapé de
pronto aparecer: texto digitado leva 0,94 / 1,36 / 1,59 / 3,23 s para ser desenhado, e o composer
leva 0,35 / 0,45 / 0,74 / 1,34 s para limpar depois do Enter. Na mesma sessão já aquecida: 0,04 s
e 0,15–0,19 s. Com prazo de 1,0 s o envio virava `partial`; a limpeza (`_limpar_composer`) mandava
C-u e a leitura seguinte via o composer vazio porque o Enter atrasado tinha sido processado, então
`limpou=True` afirmava "NÃO enviada" (e o `drain` reenfileiraria, duplicando). Pela rota
`/input`, 1 de 3 sessões novas falhou já na prova antes do Enter. Com prazo de 6,0 s (≈2× o pico),
4 de 4 sessões novas voltaram `sent` com a mensagem entregue.

## /history de transcript grande: anexos sem bolha

Em 27/09/2026 a sessão `native-correcoes` tinha um jsonl de 223 MB: 193 MB eram 11 mil linhas
`attachment` (170 MB só de `async_hook_response`, ~22 KB cada) que o `parse_obj` descarta, e o
transcript inteiro rendia 981 eventos. Com `limit=400` a janela do tail-read crescia até o início
e parseava 312 MB (cada janela reparseia do zero). Medido pela rota real (TestClient, sem subir
backend), 3 rodadas, mediana, arquivo em cache: `limit=400` 0,71 s → 0,36 s; `limit=120`
0,19 → 0,11 s; `limit=60` 0,066 → 0,032 s; sem limite 0,50 → 0,26 s. Corpo e ETag idênticos; 251
transcripts reais × 3 limites comparados com o atalho desligado, zero diferenças. O relógio do
anexo pulado continua contando porque decide o `ts` herdado por linha sem timestamp e por entrada
de fila sem `ts`.

## Codex: compactação e ida ao terminal

Na interrupção de 22/09/2026, o rollout registrou `turn_aborted`, mas o hook ficou em `working`:
o chat mostrou pronto e o card continuou em execução. A lista passou a consultar o mesmo retrato
do adapter usado pelo chat. Na prova com uma sessão descartável com terminal, `/interrupt`
encerrou o turno e `/api/sessions` retornou `idle` mesmo com o marcador ainda em `working`.

Em 22/09/2026, com codex-cli 0.155.1 no Linux, `/compact` enviado pelo composer produziu um
registro `compacted` no rollout e exibiu “Compactando…” até concluir. Na troca pelo botão,
a TUI retomou o mesmo UUID; `thread/resume` confirmou `on-request` e sandbox `readOnly`, e o
Codex lembrou o marcador enviado antes da troca. A tela passou a mostrar “Terminal” sem recarga.

A assinatura de eventos não pode impor Full Access: isso desfazia a permissão preservada pelo
lançador. Monitor de estado e prévia também precisam sobreviver à troca do cliente. Falha ao
abrir o pane restaura o sidecar e o processo sem terminal; thread sem rollout é recusada antes
de encerrar o processo original. A volta para sem terminal lê a política vigente na thread, cancela
o observador do pane antes de fechá-lo e confirma a saída dos processos antes de iniciar o cano.
Se o cano falhar, o terminal é restaurado; processo que não encerra impede a abertura de outro.

## Erro de leitura tratado como erro de linha trava o backend inteiro

Fechar uma sessão Codex sem terminal derrubava o backend, e "derrubava" é literal: processo vivo,
porta 8765 aceitando conexão, nenhuma resposta saindo. Aconteceu 7 vezes entre 14 e 21/09/2026
(`hangar-vigia.log`).

No Windows o cano fechado não chega como EOF — chega como `ConnectionResetError` (WinError 64).
O `_read_loop` do `adapters/codex/appserver.py` tratava qualquer `Exception` como erro daquela
linha: logava e `continue`. Mas `readline()` não lê a próxima linha — o `StreamReader` guardou a
exceção (`streams.py`: `raise self._exception`) e relevanta a MESMA, sem passar por ponto de
suspensão. Três consequências, todas medidas:

1. O loop nunca cede o event loop → o backend inteiro para de responder.
2. `task.cancel()` do `close()` nunca chega a agir — não há ponto de suspensão onde o cancel pegue.
3. `raise` do mesmo objeto APENSA frames ao traceback dele. Medido: +3 frames por volta. Aos
   ~12.500 frames, cada `logger.exception` reformatava tudo, com `ast.parse` por frame
   (`_should_show_carets`) — custo quadrático, 1,6 MB/s de log, 230 MB em minutos, e as 4
   rotações de `backend.log` consumidas no mesmo segundo levaram junto o histórico útil.

O "não sobe de novo" é consequência: uvicorn atende sinal pelo event loop, que está travado, então
o processo não morre e segura a porta — e `Restart-HangarTask` recusa subir instância nova com a
porta ocupada (`scripts/windows-tasks.ps1`), enquanto a vigia só age com o processo há mais de
10 minutos no ar.

Antes e depois no mesmo `backend.log`, mesmo WinError 64: 11:53 (antigo) 3 entradas de ~12.500
linhas cada; 11:54 (novo) uma linha INFO e o loop encerrado, backend respondendo em 8 ms.

Regressão coberta por `test_transport_error_on_read_ends_loop_instead_of_spinning`, que conta
registros de log e aborta no teto — sem isso a volta do bug travaria a suíte em vez de falhar,
já que nenhum timeout async chega a rodar num loop que não suspende.

## Troca de provider durante o SSE

Registro de 21/08/2026, movido do `CLAUDE.md` em 11/09/2026. Uma sessão Pi/Kimi recém-criada
levava cerca de 15s para publicar o bilhete do pane. Nesse intervalo o registry a classificava
como `claude`, com um caminho de transcript que nunca existiria. Na captura, o SSE abriu como
Claude às 16:01:14 e o JSONL do Pi nasceu às 16:01:31 em `~/.pi/agent/sessions/`.

Trocar só o arquivo deixava parser, monitor de estado e prévia no adapter escolhido na abertura:
o tailer lia o arquivo certo com o parser errado e o chat ficava mudo até sair e voltar.
O `jsonl_watcher` passou a observar `provider` junto de `jsonl`; `__reprovider__` refaz as quatro
tarefas dependentes do adapter. Troca de provider não espera os dois polls de confirmação
exigidos para troca de arquivo: é a identificação inicial da sessão, não uma oscilação.
O drain resolve o adapter na hora para não enviar teclas à TUI errada.

## Ponte de skills (`app/skill_bridge.py`): o omp descobre sozinho as skills dos outros CLIs (providers `claude`/`claude-plugins`/`agents`); Pi e Kimi leem as pastas da própria config.

Sem a ponte, cada um mantinha uma fazenda de symlinks à mão apontando pro
  cache VERSIONADO dos plugins (`plugins/cache/ecc/ecc/2.2.0/skills/...`): bump de versão =
  dezenas de links pendurados, calados (03/09/2026: 3 fazendas manuais, 99/119/157 links, todas
  com podres). A ponte varre as fontes (`~/.claude/skills`, `skills/` do repo, cache — só a
  versão MAIS NOVA de cada plugin —, marketplaces, `~/.agents/skills`), dedup por nome na ordem
  de precedência, e materializa symlinks nas pontes: pi → `~/.pi/agent/skills-bridge`, kimi →
  `~/.kimi-code/skills-bridge`. Harness novo = uma linha em `TARGETS`;
  o omp fica fora de propósito (descobre nativo), e o Codex tem reconciliador próprio desde
  06/09/2026 (abaixo). Regras duras: stdlib-only (o installer chama
  com o python3 do sistema, regra do `engines.py`); **só mexe em symlink cujo alvo está numa
  fonte conhecida** — arquivo real do usuário ou link à mão pra fora das fontes
  nunca é tocado; config alheia (settings.json do pi, config.toml do kimi) é só CONFERIDA, com
  aviso quando a ponte não está na lista — nunca editada. Roda na subida do backend e no
  `install-claude-wrapper.sh` (precedente `migracao_sidecars`: atualizar é `git pull` + restart,
  installer não é garantido). Standalone: `python3 backend/app/skill_bridge.py [--dry-run]`.
  **Ela é a ÚNICA dona das pastas de ponte** (04/09/2026): o `scripts/install-skills-bridge.sh`
  — que o hook `SessionStart` do Claude chama, e que o `claude-hooks-adapter` roda também dentro
  do Pi — tinha uma poda própria, só de plugins, e apagava a cada largada do Pi os 67 links de
  skills pessoais/marketplace que a ponte criava (o Pi abria listando cada uma como "skill path
  does not exist", e o backend as recriava no restart seguinte: 67 criados, todo dia). Hoje esse
  script cuida da persona do Pi/Kimi e dos pacotes do Pi, e chama a ponte no fim. Ele não escreve
  mais no Codex: nem hooks, nem persona, nem symlinks de skills.

## Integração nativa do Codex

(`app/codex_integracao.py`, `codex_importador.py`,
  `codex_compat.py`, `codex_arquivos.py`, 06/09/2026): o Hangar usa o importador oficial
  `externalAgentConfig/detect` + `import` e espera a notificação `import/completed` com o mesmo
  `importId`. Plugins e marketplaces usam os comandos nativos do CLI; nenhum turno de agente é
  aberto para sincronizar. Dois gatilhos, e só: a abertura de uma sessão Codex (o lançador chama
  `POST /api/harness/codex/integracao/sessao` e espera até 20s) e o botão **Reconciliar agora**.
  Sem laço e sem rodada na subida — decisão do usuário em 06/09/2026, no lugar da varredura das
  pastas do Claude a cada 30s que veio no PR: **Codex converte, backend decide quando, lançador só
  avisa.** A abertura é um cache por conteúdo (`precisa_reconciliar`): a assinatura das pastas do
  Claude fica no `estado.json`; igual à última, marketplace dentro das 6h e última rodada sem
  falha = o Codex nem é chamado (medido: 0,08s contra 1,0–1,3s da rodada vazia do PR). Falha só é
  refeita 5 min depois, na abertura seguinte. A sincronização opcional do Codex Desktop é
  independente e não é necessária. A documentação de arquitetura, migração e limites está em
  [`docs/codex-integration.md`](docs/codex-integration.md).
  O registro e os backups ficam em `~/.hangar/codex-integracao/<identidade>/`, separados por
  `CODEX_HOME`; o lock em `CODEX_HOME/.hangar-integracao.lock` serializa os escritores do Hangar
  mesmo quando seus valores de `HOME` diferem. `GET` do painel é só
  leitura, `POST` inicia ou acompanha a operação existente (202). O painel consulta enquanto a
  operação executa e descarta respostas ao trocar servidor/desmontar. A integração não grava
  confiança de hooks: normalizar RTK/`SessionEnd` pode invalidar aprovação, e quem confia de novo
  é o lançador, na abertura da sessão (decisão do dono, 07/10/2026: os hooks vêm da sincronização
  do próprio Hangar e a pergunta da TUI travava a abertura pelo app). Instruções globais usam bloco gerenciado no `AGENTS.md`; fallbacks `CLAUDE.md`
  e `CLAUDE.MD` são acrescentados à config sem substituir os já existentes.
  `settings.env` entra pelo item nativo `CONFIG` em HOME temporário; somente
  `shell_environment_policy.set` é mesclado por variável e registrado no manifesto. As políticas
  de herança/filtros e as demais preferências do Codex permanecem intactas. Fonte inválida ou
  conversão incompleta nunca significa remoção. Valores de tokens de ferramentas são locais e
  não devem aparecer no painel, nos logs públicos ou no Git.
  A suíte desliga apenas os gatilhos automáticos com `CP_CODEX_SYNC_ENABLED=0`; testes do serviço
  usam diretórios temporários. Turnos reais do CLI 0.153.4 responderam exatamente `OK`, rc=0,
  zero eventos de ferramentas, em Linux (6,08s) e Windows (6,82s), em 06/09/2026. Usaram
  `HOME`/`CODEX_HOME` temporários com apenas `auth.json` copiado com autorização; cópias e
  diretórios foram removidos e a limpeza confirmada. Windows usou CLI puro, e o Desktop do
  usuário não foi alterado nem exercitado. Essa prova de resposta não valida execução dos
  plugins/hooks importados: os turnos não usaram ferramentas.
  **O que a revisão do PR #2 mudou, medido em 06/09/2026 com a importação real (CLI 0.153.4) sobre
  uma cópia do layout desta máquina** — 9 plugins habilitados, 18 entradas no `hooks.json`,
  `AGENTS.md` como link pro `CLAUDE.md`, 379 links de skills:
  - **A conversão dos hooks do usuário é do Codex, não do Hangar.** O importador descarta o que
    não conhece (`MessageDisplay` e `Notification` sumiram sozinhos) e copia cada script pra
    `~/.codex/hooks/`. O Hangar só faz o que ele não faz: `codex_compat` (rtk, `SessionEnd` ≤ 3s,
    bloco do `AGENTS.md`), a ponte de skills pessoais e os plugins.
  - **Os hooks do PRÓPRIO app não atravessam pelo importador** (`sem_hooks_do_app`): cada harness
    recebe o `state_hook` pelo instalador dele — `codex_hook_installer.py` no Codex, irmão do do
    Kimi —, e `adapters/codex/adapter.py` lê esse marcador como segunda fonte de "turno fechou",
    então ele precisa existir mesmo com a integração desligada. Sem o filtro, `askq_capture`,
    `preview_hook`, `pair_hook`, `nav_hook` e `subagent_hook` (que só entendem o stdin do Claude)
    iam junto. O instalador preserva entradas funcionais: reescrever o comando muda o hook, e hook
    alterado é hook não aprovado no Codex.
    O `guard_tmux.py` também é do app, embora no Claude seu caminho fique sob `~/.claude/hooks`
    para atender à allowlist do Pi. Em 13/09/2026 ele atravessou o importador com a forma
    `"python" "guard_tmux.py"`: no PowerShell usado pelos hooks do Codex 0.154.0, o comando saiu
    com código 1 em cada `PreToolUse`. O instalador do Codex agora fornece a entrada própria em
    forma PowerShell e preserva `$LASTEXITCODE`, pois o código 2 é o contrato que bloqueia uma
    derrubada do servidor. A migração troca somente o `guard_tmux.py` antigo; hooks pessoais
    continuam intocados e nenhuma confiança é gravada automaticamente.
  - **A primeira rodada adota o que o instalador antigo escreveu** (`_migrar_ponte_antiga`): o
    espelho `~/.codex/.hangar-hooks.json` é o registro exato do que `install-skills-bridge.sh`
    gravava, então ele diz o que sai, sem chute. Sem isso a máquina ficava com cada hook em
    dobro — 18 entradas viraram 37 na primeira rodada (a antiga em `~/.claude/hooks/` e a cópia
    nova em `~/.codex/hooks/`), `sync-skills.sh &` e `state_hook` 2× por evento. Depois: 17 (11
    do usuário + 5 de estado + rtk), segunda rodada em 1,0s sem reescrever nada. O instalador da
    subida já rodou quando a migração tira a entrada antiga, por isso ela reinstala na hora.
  - **O rtk embrulhado reusa o interpretador e o wrapper já gravados** (`wrapper_instalado`):
    `sys.executable` + o checkout de quem reconciliou reescreviam o comando a cada backend
    subindo de outra árvore (medido: worktree `.worktrees/pr2` no `hooks.json`), e cada
    reescrita invalida a aprovação.
  - **`AGENTS.md` como link pro `CLAUDE.md` vira arquivo com o bloco** — decisão do usuário: o
    Codex lê o `CLAUDE.md` pela instrução, e o `CLAUDE.md` nunca fica cristalizado numa cópia.
    Custo: as instruções globais deixam de estar no contexto desde o primeiro token.
    **Superada em 07/09/2026** pelo `AGENTS.override.md` (ver "Instruções nativas" neste arquivo):
    o bloco sai e o `CLAUDE.md` entra inteiro, por link, no primeiro request.
  - **O gatilho de sessão nasce ligado, com interruptor na tela e sob o kill-switch**
    (`sincronizacao_ligada`): `codex_sync` no `runtime-config` (card do Codex em Harnesses) +
    `automations_enabled()` + `CP_CODEX_SYNC_ENABLED` (desligamento duro, o da suíte). O botão
    "Reconciliar agora" não passa por nenhum dos três.
  - **Quem reconcilia é o backend; o lançador da TUI pede, espera até 20s e abre** (o PR fazia o
    lançador reconciliar sozinho, esperando o lock sem prazo — com uma instalação de plugins de
    65–103s o pane ficava minutos parado, e um teto que cancelasse a rodada nunca a deixaria
    terminar). Um executor só, e a instalação longa termina no backend.
  - **`~/.agents/skills` não é do Codex** (`codex_skills._duplicata_nativa`): é fonte do Pi, do
    Kimi e do omp, e o Codex a lê sozinho. A dedupe do PR apagava dali qualquer cópia idêntica à
    fonte do Claude, com ou sem plugin nativo envolvido — nesta máquina são 11 skills pessoais que
    existem nos dois lugares, e sumiriam dos outros três harnesses, caladas. Regra: skill que já
    está em `~/.agents/skills` não ganha link na ponte (o Codex já a vê); a dedupe só roda com
    plugin nativo confirmado e só retira o que o manifesto diz que o Hangar mesmo pôs lá; cópia
    pessoal fica, com aviso. Medido na cópia fiel desta máquina: a ponte vai de 375 links pra
    42 (só o que não vem de plugin nem de `~/.agents/skills`), 333 nativas, os 11 de
    `~/.agents/skills` intactos e sem link, zero avisos.
  - **Erro fora dos três tipos esperados deixava o estado preso em "executando"**: `hooks/list`
    num formato inesperado dava `AttributeError`, escapava do `except`, e o botão ficava cinza e o
    lançador esperava 20s a cada sessão até reiniciar o backend. Hoje qualquer exceção vira
    "erro" (detalhe só no log) e o formato do `hooks/list` é conferido antes de percorrer.
  - **Um `.md` que o Codex não reconhece não derruba a etapa** (medido no CLI 0.153.4: o
    detector aceita qualquer `.md` em `commands/`, inclusive sem frontmatter e em subpasta, mas um
    `README.md` em `agents/` fica de fora). O PR abortava hooks, env, MCPs e agentes inteiros quando
    a contagem não batia, toda rodada. Hoje o arquivo não reconhecido entra num aviso, é ignorado,
    e o artefato que já existia com aquele nome não é podado.
  - **Mensagem pra tela é código + parâmetros** (`app/codex_msgs.py`, `CATALOGO`; o front traduz
    por `harness_codex_m_<codigo>`). Uma `Mensagem` É uma `str` — log, lançador e testes seguem
    lendo o texto —, e `status()` a serializa em `{codigo, params, texto}`; código que o app não
    conhece cai no `texto`. Armadilha medida: `copy.deepcopy` numa `str` com `__new__` próprio
    reconstrói pelo VALOR (`KeyError: 'Concluído'`), daí o `__reduce__`/`__deepcopy__`.
  - `AbortSignal.any` só existe do Safari 17.4 em diante (`credenciais.ts:comTeto`); sem o
    fallback, um iPhone mais velho derrubava toda chamada de credenciais/harness.
  - O card mostra `skills: N na ponte, M nativas` do manifesto, no lugar do item "ponte de skills"
    que o PR tirou — sem isso, com a sincronização desligada ninguém via as skills paradas.
  - `settings.env` vai inteiro pro `shell_environment_policy.set` — as 16 variáveis desta
    máquina, 6 delas tokens (Grafana, Jira, Jenkins, Outline, ElevenLabs), também no
    `estado.json` do manifesto (0600). É o comportamento do importador nativo; o que o Hangar
    acrescenta é fazê-lo sozinho, daí o interruptor.

## Loop runner

(`app/loop.py` + `components/LoopSheet.svelte`): loop autônomo por sessão —
  goal → sessão trabalha → idle dispara tick (`_on_hook_transition`, dentro do `_work`, só com
  `sent == 0`) → roda `check_cmd` (exit 0 = `done`) ou procura `LOOP_DONE` (→ `done_claimed`,
  que SÓ fecha com confirmação humana via `/loop/resolve`) → senão re-prompta com a cauda do erro.
  Sidecar em `.hangar-loop/<nome>.json` (sobrevive `/clear`); guardrails: max_iters,
  branch≠main, kill-switch `automations_enabled`, anti-estagnação (mesma cauda 2×). Loop ativo
  **suprime o chain** da sessão. Campos `loop_status/loop_iter/loop_max` fluem no `/api/sessions`
  e no `sig` do SSE (badge 🔁 nas 2 views). Spec/decisões: docs/superpowers/specs/2026-07-22-*.md.

## Model engines

(`app/engines.py` + `app/engine_probe.py` + `components/settings/MotorForm.svelte`,
  aberto de dentro do card da chave em `ContasSettings.svelte`, tela "Contas e modelos" — a tela
  Motores foi fundida nela em 05/09/2026, pela spec de config por assunto):
  a session can run on a non-Anthropic provider — only env vars change inside that session's process,
  `~/.claude` (skills, hooks, transcript) stays the SAME. Single source of truth at
  `~/.claude/engines.json` (0600). Four invariants: (1) `engines.py` is **stdlib-only** — an
  `app.config` import there would pull in pydantic and break `scripts/hangar-engine`, which the shell
  calls with the system `python3`; (2) it's `ANTHROPIC_AUTH_TOKEN`, **never** `ANTHROPIC_API_KEY`
  (that one writes `customApiKeyResponses` into the global `~/.claude.json`); (3) the env is applied
  by `hangar-engine --exec <engine> -- claude …` (`os.execvpe` inside the pane) and **never** via
  `tmux -e`, because the key would land in `/proc/<pid>/cmdline`, world-readable — tmux doesn't
  inherit the caller's env, so there's no "just export it" path; (4) the context-window var is
  `CLAUDE_CODE_MAX_CONTEXT_TOKENS` — `CLAUDE_CODE_AUTO_COMPACT_WINDOW` measured inert on both
  providers tested, and without the right var Claude Code still compacts at ~167k on a 500k model.
  A live session's engine is read back from `/proc/<pid>/environ` (`CP_ENGINE`), same trick as
  `CLAUDE_CONFIG_DIR` — it's what keeps both resumes (`registry.resume` and the Archive one in
  `api.py`) from silently switching engines mid-conversation; Archive resume, unlike a live resume,
  has no process left to read, so it always re-asks. Models and context window come from
  `GET {base_url}/v1/models` — no static catalog, because the value varies by the user's
  subscription tier. The statusline only hides `💵`/cost-sidecar writes on an engine session — the
  effort chip (`(high✦)`) is untouched, it's not faked.

## `hangar-preview open` com a sessão FORA da tela

(`sse.nav_*`, `sessionsStore` →
  `lib/navPelaLista.ts`, `hangar:nav-open` com `oculto`, 05/09/2026). O pedido do agente era um
  `pop` em memória entregue ao PRIMEIRO stream da sessão que passasse: o celular lendo a mesma
  sessão comia o evento (medido: 7 conexões da VPS contra 2 locais) e o desktop nunca via; e
  reiniciar o backend perdia o pedido. Hoje é um marcador `{url, ts}` por sessão em
  `~/.hangar/nav/pendentes.json`, entregue **uma vez por conexão** nos dois streams — o da sessão
  e o da **lista**, que é o único que o desktop mantém aberto o tempo todo — e apagado quando o
  shell confirma que criou o view (`DELETE /nav`) ou em 10 min. Do lado do shell, o view nasce
  **escondido** (`setVisible(false)`) e já carrega: o agente dirige por CDP na hora, e o
  `NavegadorPane` só reexibe quando o usuário abrir a sessão. View já visível não é tocado pelo
  pedido oculto — ali quem manda é o painel montado. Trocar `main.cjs`/`preload.cjs` exige
  reabrir o app desktop; o front e o backend não. Três limites medidos no teste de ponta a ponta:
  (1) **um view escondido é uma página de 0×0, e isso era pior do que "não dá pra tirar print"**
  (medido 05/09/2026, Electron 43.3.0): `setVisible(false)` zera a viewport da página —
  independente dos bounds, que não a movem —, então `matchMedia("(max-width:600px)")` responde
  **true** e o agente que abria com a sessão fora da tela lia e clicava no layout de **celular**
  do app achando que era o de desktop; e não havia quadro, com `capturePage` **rejeitando**
  `UnknownVizError` (não devolvendo imagem vazia) e `Page.captureScreenshot` pendurando. O que
  desamarra a página do compositor é `Emulation.setDeviceMetricsOverride` (1280×800): a viewport
  volta, a media query volta pro desktop e o `captureScreenshot` responde em ~60ms — o print de
  view escondido passou a existir. Três regras que caíram junto: a emulação **só pode entrar com
  a página carregada** (aplicá-la no `about:blank` de um view recém-criado derruba o processo com
  **SIGSEGV**, reproduzido 3×, e é por isso que `avisarOculto` espera o `did-finish-load`);
  `capturePage` e `captureScreenshot` **não são intercambiáveis** — o primeiro serve o view
  visível, o segundo o escondido, e usar o segundo sem a emulação é o que pendura; e
  `setBackgroundThrottling(false)` **não tem efeito nenhum** aqui (medido: A, B e C da sonda
  saíram todos vazios), o que descarta portar o `acquireAgentWake` do Superset — lá o webview
  fica visível e parqueado, aqui o view é desligado no compositor. Quem sabe do estado é o
  controlador (`definirOculto`), porque é ele que já reaplica emulação depois de navegar;
  (2) a sessão que ganhou navegador fora da tela entra
  direto na aba **Navegador** ao ser aberta (`DesktopSessionContext`, só quando ela nunca
  escolheu aba); (3) **tudo isso é do layout desktop**: com a janela do Electron abaixo de 820px
  (estava com 757px numa tile do Hyprland) o app está no layout de celular — sem sidebar, sem
  stream da lista, sem `NavegadorPane` — e o pedido só marca o store. O `dist` novo ainda passa
  pelo service worker: reload comum serve o bundle velho, é Ctrl+Shift+R.

## Modo de permissão troca COM a sessão trabalhando

(`api._guard_perm`, `permission_mode.py`,
  medido 05/09/2026): BTab é tecla, não texto — com `✻ Ebbing… (6s · thinking)` na tela, um BTab
  levou de bypass pra auto na hora, como no Pi. O guard de "está trabalhando"
  (`terminal._require_drivable`) existe pro `/model`, que é TEXTO e cairia no campo de entrada
  como mensagem; aplicado à permissão ele recusava com 409 o que o terminal aceita. O que resta
  no guard é menu aberto no pane (engoliria a tecla) e painel de terminal aberto. Do lado do
  app: o atalho (Alt+Shift+P, e Shift+Tab com foco no campo — a tecla do terminal) **sonda** o
  ciclo quando não há cache; antes pedia sem `sondar`, recebia `[]` e morria calado até a
  pílula ser aberta uma vez (0 POSTs em 7 dias de log). Ctrl+L foca o campo de qualquer lugar.
  Ciclo: sessão nascida em bypass tem 5 posições (bypass → auto → manual → acceptEdits → plan);
  as outras, 4 — bypass nunca é alcançável de fora, e `dontAsk` não tem volta.

  **Tempo da troca (07/10/2026, Claude Code 2.1.292, issue #101).** O rodapé mostra o modo novo
  15 a 27 ms depois do `send-keys BTab` (6 trocas medidas num tmux isolado). O `trocar_modo` dormia
  0,3 s antes de ler e lia a cada 0,2 s: chamando `trocar_modo`/`listar_modos` contra um Claude
  real, sem backend, a troca de um modo levava 505 ms, a de três 1,5 s e a sonda 2,6 s. Sem a pausa
  e com a primeira leitura em 20 ms: 25 ms, 71 ms e 285 ms. O intervalo cresce até 0,2 s, para pane
  lento ou morto não custar uma captura a cada 20 ms até o teto de 2 s por tecla. Na VM Windows,
  pela API: sonda de 2,7 s para 0,77 s, um modo de 1,0 s para ~0,5 s, três de 2,1 s para 0,79 s; o
  resto é o custo fixo do pedido (empréstimo do teclado e conferência do vínculo a cada tecla).
  Ajuste feito no Python porque a rota ainda não migrou para o Rust; sai junto quando migrar.
  No app nativo o Shift+Tab mostra o modo pedido na pílula na hora; teclas que chegam com a troca
  em voo, ou durante a sonda, andam o alvo e viram um pedido só, e a pílula segura o modo novo até
  o SSE trazê-lo (3 s no máximo). Antes elas eram descartadas enquanto a troca anterior não voltava.

## Plano sem terminal: a permissão vem do modo de BASE, não do plano

(`claude_headless/adapter._plano_sem_perguntar`, `permission_mode.modo_da_conta`,
  `registry.create`, medido 15/09/2026): o modo da CLI é um só — entrar no plano tira o bypass.
  Medido numa sessão sem terminal criada com `--permission-mode plan`, com prompt que pedia pra
  ler um arquivo e gravar outro: o `Read` passou sem cartão, a gravação fora da pasta de planos a
  própria CLI barrou (nem chegou a perguntar) e o único `can_use_tool` do turno foi o
  `ExitPlanMode`. Segunda sonda, negando tudo pra ver só O QUE ele pergunta, com três comandos de
  shell no prompt: `curl -s -o /dev/null -w '%{http_code}' https://example.com` rodou direto (200,
  sem cartão), e `touch` e `git init` a CLI recusou sozinha — *"Cria arquivo, escrita bloqueada no
  plano mode"* —, de novo zero `can_use_tool`. Em plano, portanto, o que chega ao
  `--permission-prompt-tool` é a saída do plano, não a ferramenta. Ou seja, o cartão por ferramenta que aparecia não era do plano: era do modo de
  base. Nesta máquina `~/.claude/settings.json` traz `permissions.defaultMode:
  "bypassPermissions"`; numa que não define nada, a sessão criada com **Permissão: padrão** nascia
  sem a flag (o campo ia nulo) e caía no manual da CLI — aí cada ferramenta pede, em plano ou fora
  dele. Foi o que apareceu no Windows.

  Duas decisões, e elas se sustentam juntas: (1) "padrão" na tela vira o modo da conta AINDA na
  criação, lido do `settings.json` dela, com `bypassPermissions` quando a conta não define nenhum —
  é o que o app já dá aos outros agentes (`lancador.SANDBOX`); (2) em plano, sessão cujo modo de
  base é bypass tem todo `can_use_tool` respondido com `allow` na hora, menos o `ExitPlanMode`,
  que é o cartão do plano e continua sendo a sua conferência, e menos as ferramentas de escrita
  (`Edit`/`Write`/`MultiEdit`/`NotebookEdit`) — hoje elas nem chegam a perguntar, e a lista existe
  pra que uma CLI futura que pergunte receba um cartão em vez de um "sim" calado. Sessão que nasce JÁ no plano grava o
  modo de base em `previous_non_plan` — sem isso ela não teria base nenhuma e a regra (2) nunca
  valeria justamente pra quem abre no plano.

  Conferido ao vivo depois da mudança: mesmo prompt, `awaiting_input` com "Permitir ExitPlanMode?"
  e nenhum cartão de ferramenta.

  "Cartão do plano", porém, era só o nome: o `can_use_tool` do `ExitPlanMode` virava a permissão
  genérica, e o card do front só existia com a sessão `idle`. Resultado medido em 14/09 e 16/09
  (sessões sem terminal de um projeto web, contas diferentes): a pessoa aprovou "Permitir
  ExitPlanMode?" sem ver plano nenhum e só depois perguntou onde ele estava. A pasta de planos
  não era a causa — `plano_claude.descobrir` achou o arquivo nas duas contas. O pedido da CLI
  (2.1.273) já traz `input.plan`, `input.planFilePath` e `tool_use_id`; agora o estado publica
  isso em `claude_plan_pending`, a pergunta vira "Aprovar o plano?" com "Aprovar plano" /
  "Continuar planejando" (negar pede pra seguir planejando), e o card ancora na chamada do
  `ExitPlanMode` só com "Ver plano" — aprovar continua sendo o seletor, um caminho só.

  Aprovar também não devolvia a base. Na CLI 2.1.273 o `ExitPlanMode` sai para
  `prePlanMode ?? "default"`, e sessão que NASCEU no plano não tem `prePlanMode`: caía em
  `default` e pedia cada `Edit`, mesmo aberta em bypass (medido ao vivo, 16/09). Duas tentativas
  que não bastaram, medidas: `updatedPermissions: setMode` na resposta da permissão é ignorado
  (a ferramenta roda depois e sobrescreve); `set_permission_mode bypassPermissions` depois da
  saída é recusado — *"the session was not launched with --dangerously-skip-permissions"*.
  O que funciona: base bypass sobe com `--allow-dangerously-skip-permissions` (só essa base,
  e só quando o modo de nascença não é o próprio bypass), e o `select` que aprova guarda a base;
  quando o `system/status` anuncia a saída do plano, o adapter reaplica a base por
  `set_permission_mode` (grava o sidecar). Conferido ao vivo: aprovou, "Modo: Bypass", o README
  editado sem nenhum cartão de permissão.

## Codex: provedor fora do ar, o turno nunca fecha

Medido em 21/09/2026, codex-cli 0.154.0, `app-server --stdio` com `model_provider` apontando para
uma porta local. A sessão `tardis-control` nasceu em 19/09 com o `jev-gateway` (`127.0.0.1:8790`)
na linha de comando; o gateway saiu em 20/09 e ela ficou dois turnos "trabalhando" sem resposta
(um de 638s, até ser interrompido). O `map_state` descartava tudo que o Codex dizia a respeito.

- **Porta que recusa a conexão:** `error` com `willRetry: true`, `message: "Reconnecting...
  waiting for network"`, `additionalDetails: "Connection failed: error sending request"`, em
  10s, 19s, 32s, 55s, 98s, 161s, 224s — sem teto. Nunca vem `turn/completed`.
- **Porta que responde erro HTTP (501):** `error` "Reconnecting... 1/5" a "5/5" com `willRetry`,
  depois `thread/status/changed` `systemError`, `error` final e `turn/completed` com
  `turn.status: "failed"` e `turn.error.message`, tudo em ~31s. O `additionalDetails` é a página
  HTML inteira do provedor — por isso só a primeira linha vira detalhe.

Sem terminal não há TUI mostrando o "Reconnecting", então a faixa é o único lugar onde isso
aparece — e ela leva o botão Reiniciar (`POST /recarregar`, que no Codex vale com a sessão
trabalhando: mata o cano e sobe outro na mesma thread).

Reiniciar sozinho não bastava: `thread/resume` de uma thread cujo provedor saiu da config responde
`-32600 failed to load configuration: Model provider `x` not found`, e a subida desistia em três
tentativas. Com `"modelProvider": "openai"` no mesmo pedido a thread volta e o turno seguinte
responde. O `-c model_provider=…` da linha de comando do processo antigo não sobrevive à subida
nova, que é montada pelo `sem_terminal` de hoje.

## Codex: Stop, queda no meio do turno e pensamento (medido 07/10/2026)

codex-cli 0.160.1, `app-server` em stdio, `gpt-5.6-luna`, conversa descartável. Os três métodos
abaixo só aparecem no esquema com `generate-json-schema --experimental`; o Hangar já pede
`experimentalApi`.

- **Stop:** turno rodando `sleep 3017`; `turn/interrupt` respondeu, o turno fechou `interrupted`
  e, 3 s depois, o `sleep` seguia vivo. `thread/backgroundTerminals/list` listou o comando com
  `processId: "43041"` (id do Codex, `osPid` nulo); `terminate` respondeu `terminated: true` e o
  processo sumiu. O T3 Code faz o mesmo e só para os comandos do turno interrompido.
- **Queda:** `SIGKILL` no grupo do app-server com o comando rodando; outro app-server, `thread/resume`
  e `thread/read` com `includeTurns`: conversa `idle`, turno `interrupted`, sem erro. É a mesma
  marca de um Stop, por isso a regra exige que a vida anterior estivesse trabalhando.
- **Pensamento:** os rollouts recentes desta máquina tinham `reasoning` com `summary: []` e só o
  `encrypted_content`. Com `summary: "detailed"` no `turn/start` chegaram
  `item/reasoning/summaryPartAdded` e `item/reasoning/summaryTextDelta`, e o rollout gravou
  `summary: [{"type":"summary_text","text":"**Confirming 391 is composite**"}]`. No luna o resumo
  é uma linha; modelos maiores escrevem mais.

## Codex sem terminal: `thread/start` leva o modelo, o esforço precisa de outro pedido

(`codex/adapter._subir_sem_terminal`, medido 15/09/2026): o `ThreadStartParams` do app-server tem
  `model` e **não** tem campo de esforço — quem tem é o `TurnStartParams`, e o `send_prompt` não
  manda escolha nenhuma de propósito (com TUI, reenviar sobrescreveria uma troca feita no
  terminal). Resultado: o nível escolhido na tela de criação era descartado calado e a thread
  ficava no `model_reasoning_effort` do `config.toml`. Sonda contra o app-server: `thread/start`
  com `model=gpt-5.6-luna` aplicou o modelo e devolveu `reasoningEffort: medium`, o do config.
  Conserto: um `thread/settings/update` logo depois do start/resume, com modelo e nível do
  sidecar. Ao vivo, sessão criada com `gpt-5.6-luna`/`xhigh` gravou
  `turn_context: gpt-5.6-luna xhigh` no rollout.

  Esse update falhar NÃO derruba a sessão: a thread já está aberta, e trocar uma escolha perdida
  por uma sessão inexistente seria pior. O nível fica o do `config.toml` e a sessão carrega
  `codex_esforco_nao_aplicado` como problema visível — perder a escolha calado é justamente o bug
  que esta entrada conserta.

## Modelo de uma sessão Claude Code: a lista NUNCA é constante

(`app/model_picker.py` +
  `terminal_input.list_model_options` / `set_engine_model` + `app/default_model.py` +
  `components/ClaudeModelPopover.svelte` + `components/ClaudeEffortPopover.svelte`). Duas fontes, escolhidas pelo que a sessão é — medido em
  31/07/2026, claude 2.1.220:
  - **Conta Anthropic** → as linhas do próprio picker do `/model`, lidas ao vivo (abre, parseia,
    Esc). A lista `['default','opus','sonnet','haiku']` chumbada no front envelheceu: o picker real
    tem 5 linhas com o **Fable** entre Opus e Sonnet, então o app escondia um modelo e ainda dava a
    Sonnet/Haiku o número de linha errado. `MODEL_NUMBERS` sobrou só como fallback pra linha rolada
    pra fora da viewport. Cache de **1 hora** por config dir porque ler a lista **dirige o
    terminal** e isso deixa rastro: `❯ /model` + `⎿ Kept model as …` (o Esc de saída) ficam no
    scrollback do tmux pra sempre. Não polui o chat do app — entra no jsonl como `type: system`,
    que o `transcript.py` ignora —, mas quem estiver com aquele terminal aberto vê, e cinco
    leituras empilhadas ali já pareceram bug. Esperar o **rodapé** (`Esc to cancel`), não só o
    título, antes de parsear: no instante em que o título aparece as linhas ainda estão sendo
    pintadas e a leitura devolvia 4 modelos, sem o Haiku. E nunca mandar o 2º Enter sem antes
    reler: se o picker já abriu, esse Enter **confirma como default** a linha sob o cursor — num
    caminho que era pra ser só leitura.
  - **Sessão de motor** → o `/v1/models` do provedor (o mesmo `engine_probe` do "Testar e listar
    modelos" de Contas e modelos).
    Ali o picker é inútil: lista os 4 aliases, **todos apontando pro mesmo `ANTHROPIC_MODEL`**
    (`Custom Opus model`, `Custom Fable model`, …) — e `gateway_model_discovery: true` não muda
    isso. A troca vai por `/model <id>`, que aceita id arbitrário.
  Três armadilhas medidas: (1) `/model <id>` grava o id como **default GLOBAL** no
  `settings.json` ("saved as your default for new sessions") — uma sessão nova da conta Anthropic
  nasceria pedindo `kimi-for-coding`; `default_model.restore_quando_aterrissar` repõe o valor
  anterior, e **espera a escrita chegar**, porque o arquivo só muda ~0.8s depois do Enter (repor
  antes é um no-op e o vazamento aterrissa em seguida); (2) a linha `⎿ Set model to …` da troca
  ANTERIOR continua na tela, então a confirmação só vale se **mencionar o id pedido** — sem isso a
  primeira leitura devolvia a resposta da troca passada como se fosse desta; (3) o guard de "posso
  digitar agora?" usa **duas capturas**, não uma: um pane parado não distingue spinner vivo de
  marcador de turno concluído (está na docstring do `state.classify`), e uma captura só recusava,
  com "está trabalhando", uma sessão que tinha acabado de terminar.

## As extensões de FUNCIONAMENTO da experiência Claude no Pi moram aqui

(`scripts/pi/`,
  04/09/2026): `claude-bridge.ts` (agents/commands/skills do `~/.claude` como recursos do Pi),
  `claude-todo.ts` (painel de tarefas), `claude-hooks-adapter.ts` (hooks do `settings.json` nos
  eventos do Pi), `git-checkpoint.ts` (`/rewind`) e `fullscreen-tui.ts` (alternate screen no Pi)
  vieram do repo `pi-claude-bridge`, que ficou só com aparência (caixa da mensagem, título do
  terminal e temas). Motivo: sem elas uma sessão Pi criada pelo app não enxerga skills/agents nem
  roda hooks, e quem instala o Hangar não deveria precisar de um segundo repo pra isso. O
  `install-claude-wrapper.sh` symlinka as sete no Pi e cinco no OMP: neste, `claude-todo` e
  `fullscreen-tui` ficam com o núcleo. O painel de saúde usa a mesma seleção por CLI e não
  oferece fullscreen no OMP. No Pi com fullscreen nativo, a extensão não assume o buffer
  alternativo para evitar dupla posse.
  **Comparação com OMP 18.1.11 (05/09/2026):** em uma HOME descartável, sem chamadas a modelos,
  `getAllTools()` mostrou que `claude-todo` substituía a ferramenta `<builtin:todo>` pela extensão:
  o contrato `action/id/activeForm` tomava o lugar de `op/task/phase`, fases e bloqueios usados
  pelo próprio núcleo. Com a proteção no entrypoint, a origem continua `builtin`. A proteção
  também cobre quem atualiza só com `git pull`, sem rodar o instalador.
  Na mesma prova com o binário real e sockets tmux separados, o nativo usou
  `alternate_on=0, mouse_any_flag=0`; fullscreen externo ativado usou `1,0`. Após a correção,
  mesmo carregado explicitamente com `enabled: true`, permaneceu `0,0`. O OMP depende do
  scrollback normal; colocar a conversa no buffer alternativo não cria um renderizador com
  rolagem (issues can1357/oh-my-pi#10232 e #2040).
  **Migração não edita preferências:** instalador e reparo removem somente symlinks próprios
  dessas duas extensões. Arquivos reais, symlinks para outra fonte e `fullscreen-tui.json`
  ficam intactos. A configuração do OMP não é reescrita.
  **Não estender essa conclusão às demais extensões.** A descoberta nativa do OMP respeita
  registros/escopo de plugins e reduz a necessidade de espelhar skills e comandos, mas não
  importa todos os agents pessoais do Claude, sua normalização de modelos nem o índice de
  memória. Hooks JS/TS nativos não executam automaticamente o protocolo CLI do `settings.json`.
  E checkpoint/rewind nativos reduzem contexto: não restauram arquivos como o shadow Git do
  Hangar. Essas capacidades continuam complementares, não substituídas por nome.
  A seleção das extensões não demonstra compatibilidade completa: o bridge e os checkpoints
  têm as provas específicas abaixo; limitações do adaptador de hooks continuam separadas.
  **Bridge adaptado ao OMP (05/09/2026):** `lib/agent-context.ts` resolve a identidade pelo
  executável e normaliza `PI_CODING_AGENT_DIR`/`CLAUDE_CONFIG_DIR`, incluindo `~`; a fábrica
  guarda esse contexto por instância. **Os helpers compartilhados moram em `scripts/pi/lib/` e
  o instalador (e `harness_saude._ligar_extensoes`) linka a PASTA `extensions/lib`** (medido
  06/09/2026, pi 0.85.0): o loader do Pi resolve import relativo pelo caminho do symlink, não do
  arquivo real — `./agent-context` ao lado de `claude-bridge.ts` dava `Cannot find module` e o
  Pi saía com rc=1 sem ponte nem `/rewind`; e um `.ts` solto em `extensions/` é carregado como
  extensão (`does not export a valid factory function`). Pasta sem `index.ts` o Pi ignora. O omp
  (Bun) resolve pelo realpath e carregava de qualquer jeito — foi por isso que a suíte, que só
  roda o omp, não pegou. **"Estou no omp?" tem UMA resposta**, `getAgentContext().harness`:
  `claude-todo.ts` e `fullscreen-tui.ts` tinham o regex do `execPath` copiado, e uma mudança de
  empacotamento do omp corrigida no `lib/` deixaria as duas religando no omp o que tem que ficar
  desligado. Miudezas fechadas junto (06/09/2026): nome de agente repetido entre fontes no omp
  entra em `skipped` (o caminho lá é plano, o segundo era descartado calado); o desfazer de uma
  ação do plugin sync que falha loga e relança a **causa original** (antes o `finally: raise`
  punha o erro do desfazer no relatório); `_digest` guarda assinatura (mtime, tamanho) por
  arquivo e só relê quando ela muda (o laço de 300 s relia todo byte de todo plugin); e
  `observe_controls` lê as 3 chaves do `omp config get` em paralelo e a releitura só pega
  `disabledExtensions` — de 6 processos em série (~0,75 s cada, na subida do backend) pra 4 em
  dois lotes.
  **A raiz do agente omp tem UMA resposta: `app/omp_dirs.agent_dir()`** (06/09/2026). O omp
  com perfil (`--profile x` ou `OMP_PROFILE=x`) grava TUDO — login, sessões, config, plugins —
  em `~/.omp/profiles/x/agent` (medido no 18.1.10 numa HOME descartável). O plugin sync e o
  contexto vieram com `resolve_omp_directories`, que espelha essa regra; sessões
  (`sessions_root("omp")`), painel de saúde (`_raiz_agente`) e login do ChatGPT (`_omp_db`)
  continuavam em `~/.omp/agent` sem perfil — com `OMP_PROFILE` no ambiente do serviço, o sync
  instalava no perfil e o painel dizia "não instalado". Hoje os três perguntam ao `omp_dirs`,
  que só embrulha o resolvedor do sync (import tardio: `sessions.py` é folha e não pode puxar
  `peers` na importação) e, pra quem só LÊ, perfil inválido vira aviso e raiz sem perfil —
  levantar ali derrubaria a listagem de sessões inteira.
  **Perfil por SESSÃO** (06/09/2026): o perfil de um pane omp viaja como `OMP_PROFILE` no
  ambiente dele — o wrapper (`omp.posix.sh`/`omp.fish`, `hangar_omp_perfil`) lê `--profile x`
  da linha ou a variável já exportada, monta o `--session` na raiz do perfil e passa `-e` pro
  tmux (o pane nasce do servidor, não do shell); o app faz o mesmo pelo `env` do
  `OmpAdapter.spawn_command(perfil=...)`. Do outro lado, `registry._omp_profile_of(pid)` lê a
  variável do processo vivo (mesmo `/proc/<pid>/environ` de `CP_ENGINE`) e passa pra
  `transcript_path`/`localizar_na_raiz`, senão a varredura de `sessions/-/` caía na raiz do
  BACKEND. Entrada: `omp_profile` no `POST /api/sessions` (só com `provider=omp`, nome validado
  pela regra do próprio omp; 400 fora dele), `hangar-send --new … --provider omp --profile x`,
  e o campo "Perfil do omp" da folha de Nova sessão, que só aparece com OMP escolhido.
  Variável e não flag de propósito: `--profile` no cmdline funcionaria pro omp mas o backend
  teria duas fontes pra ler. **O que ainda NÃO olha as pastas de perfil:** o relatório de
  custo (`costs_sources.raiz_omp`) e o Arquivo de conversas mortas (`archive_providers`) leem
  só a raiz do backend — uma sessão omp criada com perfil funciona ao vivo, mas some das duas
  telas depois de fechada. Cobrir isso é varrer `~/.omp/profiles/*/agent/sessions` além da
  raiz, e ainda não foi feito. Quem GRAVA na raiz do omp (`oauth_codex._omp_db`) usa
  `omp_dirs.agent_dir(estrito=True)`: perfil inválido levanta, em vez de cair calado na raiz
  sem perfil com a credencial gravada no lugar errado. No OMP, agents pessoais/extras viram arquivos diretos
  em `<agentDir>/agents/claude-bridge-<nome>.md`, com ferramentas em array YAML: `Glob → glob`,
  `Task/Agent → task`, `WebFetch → read` e prefixo `mcp__` intacto. Agents nativos pessoais
  têm precedência; aliases Claude sem mapeamento explícito herdam o modelo da sessão.
  Isso inclui `fable`. Negações explícitas (`disallowedTools`) são subtraídas da allowlist;
  sem uma allowlist ou com negação não representável, o agent é recusado, não ampliado.
  Nomes `main`/`sub`, reservados pelo núcleo OMP, também são recusados nesse harness.
  Skills/comandos/plugins não são espelhados no OMP, nem oferecidos no menu de fontes.
  No Pi permanecem a conversão de ferramentas e o layout recursivo de agents, prompts e skills.
  `lib/frontmatter.ts` usa `Bun.YAML.parse` no OMP e carrega o parser legado somente no Pi.
  Memória respeita `enabled`, preserva blocos do prompt e não reinsere conteúdo já presente;
  `claude-bridge.json` ilegível é logado e ignorado no `before_agent_start`, nunca lançado.
  O manifesto versão 2 registra conteúdo e caminho relativo de cada arquivo gerado: atualização
  e remoção exigem os bytes originais, e conflitos são preservados e reportados. **O manifesto
  v1 é ADOTADO uma vez** (`adoptLegacy`, 06/09/2026): ele só listava nomes de prompts, e a pasta
  `agents/claude-bridge/` era inteira da ponte — os dois já eram sobrescritos e apagados por ela,
  então adotá-los lendo o disco não tira segurança nenhuma. Tratar v1 como vazio (a primeira
  versão do PR) deixava cada instalação existente com todos os arquivos em `skipped` para
  sempre, e sem volta, porque o v2 vazio já tinha sobrescrito o v1 (nesta máquina: 16 prompts
  e três pastas de agents). Escritas usam arquivo temporário + rename. **Fonte com frontmatter
  inválida pula só ela** (entra em `skipped` com o motivo) e, enquanto houver uma, a ponte cria e
  atualiza mas **não remove nada** — sem ler a fonte não se sabe qual cópia ela geraria, que era
  o risco que o abort da primeira versão evitava ao custo de um `.md` quebrado em qualquer
  marketplace derrubar o sync inteiro. Isso adapta a ponte, não substitui o instalador nativo
  de plugins.
  Prova: `tests/test_claude_bridge_omp.py` roda o OMP real com HOME própria; o driver exige
  descoberta no catálogo de `task`, grava resultado estruturado em `session_start` e encerra
  sem prompt/modelo remoto. `rc=0` sozinho não prova carregamento de extensão.
  **Checkpoints por contexto no OMP (05/09/2026):** `git-checkpoint.ts` consome o mesmo
  `lib/agent-context.ts` e registra `/hangar-rewind`; o Pi mantém `/rewind`. A captura usa
  `before_agent_start`, não `turn_start`, e persiste revisão, worktree canônica, identidade do
  Git do projeto e diretório que contém os objetos. O próprio registro é a âncora anterior ao
  pedido. Retomada/fork conservam essa origem; `getBranch` impede oferecer um ramo descartado.
  **Uma pasta de checkpoints por SESSÃO** (`<agentDir>/checkpoints/<slug do jsonl>`, reusada
  na retomada, 06/09/2026): a primeira versão do PR abria `<slug>-<uuid>` a cada ativação,
  inclusive em cada resume, e cada pasta guarda os objetos da árvore inteira — 113 pastas e
  1,2 GB nesta máquina, sem poda. O uuid existia pra duas instâncias da mesma sessão não
  disputarem o índice; hoje o índice é **por captura** (`index.<pid>.<uuid>`, apagado no fim),
  então objetos e refs (já nomeadas por uuid) convivem num bare repo só. Sufixo `-<uuid>` só
  quando a pasta com esse nome é de OUTRO projeto (`hangar-origin.json` diverge) ou não é um
  bare repo; a pasta v1 do Pi (mesmo slug, sem origem gravada) é adotada e ganha a origem.
  Restaurações continuam com índice temporário próprio, nunca o da sessão de origem.
  **A captura enumera numa chamada só**: `ls-files -t -s --cached --others --deleted
  --exclude-standard` — `H`/`S`/`M` rastreado com modo (`160000` = submódulo, fora), `?` novo,
  `R` rastreado que sumiu do disco. A primeira versão fazia um `lstatSync` síncrono por arquivo
  rastreado mais 8–9 spawns por prompt; num repo de milhares de arquivos isso travava o loop de
  eventos antes de cada mensagem. Árvore igual à da última foto **reaproveita a revisão**
  (`lastTree`/`lastRef` na sessão ativa): todo pedido ganha registro, não commit.
  **Captura lenta ou falha NÃO mata o turno.** A primeira versão chamava `ctx.abort()` no omp
  ao estourar 25 s, em qualquer erro de captura e em `agent_start`/`turn_start` com captura
  pendente — um repo grande cancelava TODO prompt. O que importa (nada tardio entra no turno)
  é o cancelamento da captura, que fica; o abort saiu. Hoje é aviso "este pedido segue sem
  checkpoint" e o turno anda; o `/rewind` só tem um ponto a menos. O Git do projeto só
  enumera arquivos/exclusões; variáveis `GIT_*` herdadas são removidas, hooks/assinatura/fsmonitor
  são desativados e atributos do shadow preservam bytes, inclusive CRLF, sem filtros de conteúdo.
  O modo de código repõe arquivos modificados/apagados, preservando os criados depois. Origem,
  projeto, revisão, ramo, diretório e ociosidade são conferidos antes da escrita; symlinks
  ancestrais ou diretórios posteriores em colisão recusam a operação.
  **O await do OMP tem prazo:** no 18.1.11 o dispatcher libera handlers após 30 s. A captura
  tem limite total de 25 s, incluindo espera na fila, e cancela os processos Git antes desse
  limite nativo (o pedido segue, ver acima). `agent_start`/`turn_start` também invalidam qualquer
  captura restante; ela não pode publicar um checkpoint tardio no turno em execução.
  Prova usa `ExtensionRunner`/`loadExtensionFromFactory` reais, sem chamada a modelo; reproduziu
  a publicação tardia ao expirar o dispatcher e passou após o cancelamento.
  **As provas do omp reusam o addon nativo da máquina** (`tests/omp_runtime._reusar_natives`,
  06/09/2026): o omp extrai `pi_natives` (~344 MB) em `~/.omp/natives/<versão>` da HOME que
  vê, e cada caso tem HOME própria — com as 3 rodadas que o pytest guarda, um `/tmp` em tmpfs
  de 12 GB lotou NO MEIO da suíte e derrubou 756 testes com `No space left on device`, todos
  longe do omp. A HOME de teste ganha um symlink `.omp/natives` (e `.cache/omp/natives`) pro
  real quando ele existe; sem ele (CI limpo) o omp extrai como sempre.
  Registros Pi antigos só são restaurados quando a sessão original, seu `header.cwd` e o
  armazenamento legado previsto demonstram a origem; não se procura um SHA por pastas alheias.
  Código e conversa são etapas separadas: falha da segunda é informada como parcial, não sucesso.
  Regras herdadas do adapter: a allowlist embutida libera só `~/.claude/hooks/` — hook que mora
  noutro lugar entra por `~/.pi/agent/claude-hooks-adapter.json`, e `allowPatterns` ali
  **substitui** a lista, não soma; e os hooks só-Claude do próprio app (`state_hook`, `askq_capture`,
  `preview_hook`, `subagent_hook`) ficam no `skipPatterns` porque o Pi tem extensão própria pra isso.
  **Catálogos e plugins nativos (06/09/2026):** `app/omp_plugin_sync.py` oferece
  `PluginSynchronizer.import_marketplaces` e `reconcile`. A importação percorre todos os
  marketplaces registrados no Claude, sem nomes especiais, e chama o gerenciador nativo do
  OMP. Confere nome/origem no registro após o comando; catálogo homônimo divergente permanece
  intacto. Importar catálogo não instala seus plugins nem migra instalações Git existentes.
  O OMP já oferece `marketplace.autoUpdate=off|notify|auto`, com padrão `notify`; a atualização
  nativa por versão do catálogo ocorre na abertura da sessão e não é duplicada pelo Hangar.
  A reconciliação de Git direto exige origem, revisão e manifesto instalável comprovados,
  preserva escopo, seleção de recursos e preferências, e suspende a gestão após alteração
  manual. Metadados Claude sem SHA tornam somente aquele candidato não verificável.
  Em atualização, prepara apenas a dependência gerenciada antes de chamar o instalador:
  isso evita arestas duplicadas no Bun quando o parser OMP não reconhece `#SHA` em host genérico.
  Uma falha só reverte essa chave se a instalação anterior ainda estiver comprovadamente
  intacta; não remove o plugin antes da atualização nem restaura cópia global antiga.
  O registro próprio fica em `~/.hangar/omp-plugin-sync.json`, com trava portátil compartilhada
  entre passagens e escrita atômica. Operação interrompida não concede autoridade de remoção.
  Todos os vínculos e estados do ledger são validados antes de chamar o CLI ou agir; registro
  malformado não é reparado por inferência e não autoriza remover um plugin. Duas identidades
  Claude para o mesmo pacote tornam o nome ambíguo durante toda a passagem, inclusive diante
  de uma terceira origem; os candidatos independentes continuam. Diagnósticos não publicam
  texto bruto de exceções de parser/I/O, que pode transcrever credenciais da entrada.
  `dry_run=True` é somente leitura de registros/manifestos: nenhum CLI, lock, cache ou ledger
  é escrito, pois até `omp plugin list` pode migrar arquivos. Provas cobrem o CLI real, Git
  Smart HTTP em loopback privado, importação genérica e preservação da instalação nas falhas.
  **Resolução de diretórios:** `resolve_omp_directories` separa configuração, agente e dados
  conforme o OMP. `PI_CODING_AGENT_DIR` não move o armazenamento global. `PI_CONFIG_DIR`,
  precedência de `OMP_PROFILE` sobre `PI_PROFILE` (inclusive vazio), override herdado e XDG
  seguem as regras nativas. A categoria XDG exige caminho existente e agente padrão; perfis
  nomeados exigem o caminho XDG daquele perfil. A resolução é lexical, usa o cwd do filho e
  não expande `~`, não segue symlinks e não cria diretórios. Cada passagem tem sua própria visão;
  mudar o destino não migra o ledger antigo: o vínculo incompatível gera diagnóstico.
  **Passagens periódicas:** `PluginSyncLoop` é criado/encerrado no lifespan do backend, sem
  serviço externo. `CP_OMP_PLUGIN_SYNC_ENABLED` é falso por padrão; intervalo positivo e finito
  em `CP_OMP_PLUGIN_SYNC_INTERVAL` (300 s). Respeita também `automations_enabled()`. A primeira
  passagem começa na subida e a seguinte espera o intervalo após a conclusão da anterior.
  Importação/reconciliação rodam em `asyncio.to_thread`; desligar aguarda o worker em voo,
  não cancela uma Future deixando o processo externo vivo. Uma parada observada fica registrada
  até terminar a passagem, mesmo que o kill-switch seja reabilitado nesse intervalo.
  `GET /api/omp/plugin-sync`, autenticado, expõe estado, horários e relatórios sanitizados.
  Prova com backend real confirmou resposta HTTP enquanto o worker aguardava, e marcador de
  teardown confirmou o encerramento cooperativo. Nenhuma página nova foi introduzida.
  **Contexto CLAUDE.md:** `CP_OMP_CLAUDE_CONTEXT_ENABLED=1` habilita a configuração na subida
  pelo módulo `omp_context`. Reusa o resolvedor nativo de diretórios, vincula APPEND_SYSTEM.md
  ao CLAUDE.md global existente e instala/reusa a regra genérica de leitura do projeto.
  Arquivo/link personalizado em conflito é preservado e informado; arquivo global ausente
  não vira link quebrado. A lista disabledExtensions é mesclada pelo CLI nativo somente com
  `context-file:project:AGENTS.md` e `context-file:user:AGENTS.md`, sem retirar outras escolhas.
  Equivalência exige corpo compatível e frontmatter comprovadamente habilitado/incondicional,
  lido pela biblioteca YAML já usada no backend. Só arquivos diretos .md/.mdc participam, como
  no provider nativo; o nome/ID vem do arquivo. disabledExtensions, ttsr.disabledRules e
  disabledProviders são conferidos sem remover bloqueios pessoais. Precondições de arquivo,
  regra e diretório são repetidas após a trava e chamadas externas; rules/ convertido em
  symlink é recusado antes de publicar a regra, preservando o alvo externo.
  O CLI pode normalizar formatos legados de configuração, como o tema escalar para theme.dark,
  mantendo a preferência efetiva. OMP real confirmou as sentinelas CLAUDE global/de projeto
  e ausência das sentinelas AGENTS no prompt antes de ferramentas; projeto sem CLAUDE não
  recebe conteúdo inventado. Outros harnesses e arquivos AGENTS.md dos projetos não são alterados.

## Pi model + thinking level

(`app/pi_models.py` + `scripts/pi/hangar-state.ts` + `components/PiModelPopover.svelte` + `components/PiEffortPopover.svelte`):
  the third mechanism, next to Claude's TUI picker and Codex's app-server, and it does **not** scrape
  the pane. Measured on pi 0.82.1: `/model` is a fuzzy-**search** list of ~300 entries (footer
  `(1/301)`, 10 rows visible) — not enumerable from the pane and not navigable by counting `Down`;
  and there is no `/thinking` command (it lives inside `/settings` → "Thinking level", a submenu).
  So the Pi extension we already ship publishes a catalog sidecar
  (`<config>/.hangar-pi/models/<jsonl-stem>.json`, same key as the state marker) and registers
  `/cp-model <provider> <id>` + `/cp-think <level>`, which the backend types with `send-keys` and Pi
  applies through `pi.setModel()` / `pi.setThinkingLevel()`. Two invariants: (1) the thinking levels
  are **per model** (glm-5.2 → off/low/medium/high/xhigh; k3 → low/high/max), so they come from the
  session, never from a constant — the static `LEVELS` tuple only rejects garbage before typing;
  (2) Pi **clamps** the level to what the model supports (`agent-session.js:1277`), so the endpoint
  re-reads the sidecar and returns what *stuck*, not what was asked (asking `max` on glm-5.2 lands on
  `xhigh`). Missing sidecar → 409 telling the user to re-run `install-claude-wrapper.sh`, never an
  empty list that reads as "no models".

## `omp` (oh-my-pi) é um FORK do Pi, e é por isso que ele engana

(`adapters/omp/` — `OmpAdapter`
  é subclasse do `PiAdapter`; wrappers `scripts/shell/omp.*`). Mesmo JSONL, mesma API de extensão,
  as MESMAS `scripts/pi/*.ts` — o que muda é pequeno e cada item já custou um bug calado. Medido em
  02-03/09/2026, omp 18.1.4 (embute `pi-coding-agent` 0.84.4):
  - **Binário ELF nativo com argv0 `omp`**, não um fork do processo `pi`. Sem a entrada própria em
    `_EXEC_PROVIDER` o pane cai no default `claude` e é casado com o transcript do **Claude** do
    mesmo cwd — a regressão que o Pi já pagou.
  - **Raiz `~/.omp/agent`**, pela env `PI_CODING_AGENT_DIR` — que é variável do `pi-coding-agent` e
    move os DOIS agentes, então ela nunca é lida como "a pasta do omp" sem se lembrar disso (o Pi
    usa `PI_CODING_AGENT_SESSION_DIR` + `~/.pi/agent`). As extensões vão em
    `~/.omp/agent/extensions/`, que não existe até alguém criar.
  - **Não existe `--session-id`** (`Error: unknown flag`) — o id é do omp (uuidv7). Quem escolhe a
    sessão é o CAMINHO: `--session <arquivo>` (não documentado) cria o transcript exatamente ali.
    Por isso app e wrapper montam `<raiz>/<slug do cwd>/<ts>_<uuid>.jsonl` e exportam
    `CP_PI_SESSION=<uuid>` junto: com o bilhete da extensão como ÚNICO vínculo, um bilhete recusado
    deixa a sessão sem transcript até o próximo `agent_start` — que só chega se alguém conseguir
    mandar prompt, o que o app não consegue numa sessão untracked. Resume é `-r <caminho>` (o id
    interno do omp não é o do nome do arquivo).
    **E o diretório do `--session` não é honrado (omp 18.1.6, medido 04/09/2026):** o transcript
    principal nasce em `sessions/-/<nome>.jsonl` (o `-` é o slug de um cwd vazio) e só os
    subagentes vão pra pasta pedida — enquanto o `getSessionFile()` que a extensão publica no
    bilhete continua devolvendo o caminho pedido, que não existe. A sessão aparecia vazia no app
    com a TUI cheia. `pi_sessions.localizar_na_raiz` procura o mesmo NOME de arquivo em qualquer
    pasta da raiz (`registry.pi_session_file` e `transcript_path`, só pro omp); o Pi honra o
    diretório e continua restrito à pasta do cwd.
  - **Eventos com outro nome:** `agent_end` e `model_changed`, não `agent_settled` nem
    `model_select`. Sem tratá-los, o estado ficava preso em `working`. E a troca de modelo pelo app
    dava falso "o Pi recusou a troca" por um motivo mais fundo, achado na revisão final: o
    `setModel` devolve `true` e o rodapé troca, mas **`ctx.model` continua o modelo velho** e o omp
    não emite evento de modelo nenhum (`model_changed` também não dispara aí; `pi.getModel` não
    existe). Publicar `ctx.model` depois do `setModel` republicava o modelo VELHO com `ts` novo, e
    o backend lê "ts novo com modelo velho" como recusa. Daí o `override` de `publishModels`
    (`scripts/pi/hangar-state.ts`): quem sabe o modelo certo é quem acabou de pedi-lo. Troca feita
    no teclado do omp não tem evento, então o `agent_start` republica quando `ctx.model.id` difere
    do último publicado.
  - **Subagente roda no MESMO processo**, sem `PI_SUBAGENT_DEPTH`, emite `session_start` com o ctx
    dele e grava em `<stem>/<NomeDoAgente>.jsonl` (o Pi: `<stem>/<taskId>/run-N/session.jsonl`).
    Sem o portão, a extensão do subagente reescrevia o bilhete do pane e o histórico da sessão
    virava a conversa do subagente.
  - **`/reload` NÃO recarrega extensão editada nem descobre arquivo novo** (medido 2×): editou a
    extensão, reabra a sessão. Mesmo aviso do Pi, só que ali o `/reload` resolve.
  - **A ferramenta de perguntar chama `ask`** (no Pi é `question`), com shape multi-pergunta e um
    cartão de resumo acima do picker, mais as linhas de descrição de cada opção — quem conta linha
    de tela pra escolher opção precisa contar essas também.
  - **Catálogo de modelos é `omp models --json`** — não há `--list-models`.
  - **Sem chip de cota:** `~/.omp/agent` não tem `auth.json` nem `models.json` (é SQLite:
    `agent.db`, `models.db`), e `cotas._chaves_do_pi` lê exatamente esses dois arquivos, do `~/.pi`.
  - **No Windows a sessão omp depende SÓ do bilhete da extensão.** `_com_env` (`adapters/omp/
    adapter.py`) prefixa `env CP_PI_SESSION=<uuid>` porque o tmux não repassa o ambiente de quem
    chama — e `env` não existe lá, então o ramo `os.name == "nt"` devolve o comando cru. Bilhete
    recusado = sessão sem transcript. O conserto existe e não foi feito por falta de medição
    naquela máquina: `tmux new-session -e` funciona no psmux (é o que `tmux._e_config_dir` já usa),
    então dá pra mandar a variável pelo `-e` em vez do prefixo.
  - **Subagente do omp não aparece no painel de Atividade.** `subagents.py` é Pi-puro por duas
    travas independentes: `_pi_agents_dir` exige que o transcript esteja sob `sessions_root("pi")`
    (o do omp está sob `~/.omp/agent`), e o leitor espera o layout `<stem>/<taskId>/run-<n>/
    session.jsonl` — o omp grava `<stem>/<Nome>.jsonl`, sem `run-N`, então `_pi_run_dir` devolveria
    `None` mesmo com a raiz certa. Lista vazia, não erro.
  - **Executor omp no orquestrar usa o inventário do Pi.** `orq_politica.inventario` chama
    `pi_catalog.listar()` sem argumento (= `pi`), coerente com "mesmo inventário, sem linha
    própria" da política. Consequência a saber: `omp models --json` pode listar menos modelos que
    `pi --list-models`, e a tela do orquestrar oferece a lista do Pi.
  - **`rich-status-line.ts` honra `PI_CODING_AGENT_DIR` também numa sessão Pi.** A raiz sai de
    `process.env.PI_CODING_AGENT_DIR || ~/.{omp,pi}/agent`, e é dela que saem `auth.json` e
    `models.json` (o chip de cota). Quem exporta a variável no shell por causa do omp muda de onde
    o **Pi** lê os dois.

## Before typing into the Pi's composer, ASK — the screen cannot tell a notice from a draft


  (`terminal_input._composer_ocupado_pi` + `pi_inbox.perguntar` + `responderPergunta` in
  `scripts/pi/hangar-state.ts`). Pi prints extension notices (`console.error`) **inside the composer
  band**, with the same ANSI as typed text; measured 22-23/08/2026, `cursor_flag` is 0 either way.
  So the anti-paste guard counted our own `[hangar-state] linha do hangar conectada` as a draft and
  every `/cp-model`/`/cp-think` came back **409 with the composer empty**. Recognizing each phrase by
  regex is whack-a-mole (`/reload` draws a fourth one no regex of ours knows), and the "compare two
  captures — a notice is static, a draft changes" upgrade the code itself proposed **was measured and
  does not hold**: a *parked* draft is static too, and the parked draft is exactly what the guard
  exists for. What answers is the Pi: `ctx.ui.getEditorText()` returns `""` with a notice on the band
  and the exact text with a draft. So the `pi_inbox` line, until then delivery-only, took a second
  verb — `{id, pedir}` out, `{id, resposta}` back. Four rules: questions live in a **separate**
  futures dict from deliveries (a delivery resolves `(ok, erro)` and a question resolves a value);
  `""` is an **answer** and `None` is absence (→ fall back to scraping, so an old extension behaves
  exactly as before); the question does **not** take `linha.lock` (that lock orders *writes*, and a
  read must not queue behind a 3s ACK); and it uses `pi_inbox.linha_de(name, pane_id)`, never the raw
  pane. Note `/reload` drops and re-raises the line — a command fired inside that ~5s window falls to
  plan B and can still 409.

## Antes de digitar no composer do Claude, ESVAZIE ele

(`terminal_input._esvaziar_composer_claude`, 12/09/2026). Caso real: o transcript da sessão
  `Lhais` gravou `pq eu quero ele atualizado/modelmodelmodelmodelTá travado aí` como **uma**
  mensagem às 14:07:50. Um rascunho anterior e restos de `/model` estavam parados no composer, o
  prompt foi digitado em cima, o Enter mandou tudo grudado (e cortado), e o reconcile, sem achar
  `Tá travado aí não?` exato, reentregou a cópia limpa às 14:08:00 — `REQUEUE … mais parecida no
  transcript=''`. A guarda anti-colagem só existia pro Pi (que adia), e o Claude ficava sem nenhuma
  porque a docstring do Pi supunha que o composer vazio do Claude "desenha glifo/placeholder" e não
  dá pra distinguir. **Medido no pane** (psmux 3.3.8, Windows 10, sessão trabalhando): vazio é só o
  `❯` entre as réguas (`\x1b[0;38;2;153;153;153m❯`), então tirar o glifo basta pra dizer "vazio".

  Decisão do dono: **apagar**, não adiar. Adiar trava a fila quando o resto é lixo; apagar custa o
  rascunho que ele esteja escrevendo no terminal, e ele escolheu isso sabendo. Três regras: sem texto
  além do glifo, nenhuma tecla (o caso normal não paga nada); `C-u` só se repete enquanto o conteúdo
  **diminui** — uma tecla sem efeito é moldura ou placeholder, porque texto digitado sempre sai com
  `C-u`, e aí desiste sem gastar o teto (`_LIMPEZA_MAX_TECLAS`), o que também cobre um ocioso que
  desenhe placeholder; composer ilegível não é tocado. Roda **antes** da foto dos placeholders de
  paste, pra um `[Pasted text #N]` velho sair junto e não virar prova falsa de entrega.
  `_limpar_composer` (envio parcial, só apaga o que é NOSSO) mantém a regra antiga: são caminhos
  diferentes.

## Rascunho do dono no guardado do Claude (05/10/2026)

Na prova da VM Windows o rascunho `rascunho do dono`, parado no composer de uma sessão com
terminal, sumiu quando o app mandou uma mensagem: o escritor Rust o apagava com `C-u`
(`initial_composer`). Nova decisão do dono: **guardar e devolver**, igual, sem Enter.

Redigitar o que a tela mostra não devolve igual. Medido no Claude Code 2.1.289 (pane de 100
colunas): uma linha longa quebra na palavra (`… com` / `folga, sim`) e o espaço da quebra some, então
quebra da tela e Enter do dono ficam iguais na captura; colagem aparece como `[Pasted text #1 +5
lines]`, sem o conteúdo. O Claude tem guardado próprio no Ctrl+S, medido no mesmo pane:

- com texto, guarda e esvazia o composer; a linha de dicas acima da régua passa a terminar em
  `› stashed` (sozinha ou depois de `ctrl+g to edit in VS Code ·` / `Ctrl+Y to paste deleted text ·`)
  e fica ali enquanto houver algo guardado;
- com o composer vazio, devolve o guardado e a marca some;
- **qualquer envio devolve o guardado ao composer**, já na primeira leitura 0,5 s depois do Enter,
  inclusive `/clear`; o rascunho de várias linhas voltou idêntico, com acento, aspas, `$HOME` e
  `\`, e o conteúdo colado voltou junto do marcador (enviado depois, saíram as 5 linhas);
- é **uma vaga só**: guardar `rascunho A`, digitar `texto novo` e apertar Ctrl+S guarda o texto
  novo e perde o A.

Daí as regras do escritor (`stash_draft`/`settle_draft`/`submit` em
`crates/hangar-server/src/terminal_input.rs`): rascunho com marca de guardado já presente adia
com `composer_busy` sem tecla nenhuma; Ctrl+S sem efeito (CLI sem guardado) também adia, com o
rascunho intacto. Depois do Enter o composer não fica vazio (o rascunho volta na hora): prova o
envio a marca sumir com o composer igual ao rascunho guardado; com guardado alheio, de conteúdo
desconhecido, a marca sumir sem o nosso texto no composer. Sem envio (`deferred`), o escritor
aperta Ctrl+S só com o composer vazio e a marca presente; com envio incerto, só espera a devolução
nativa. O desfecho (`returned`/`stashed`/`unverified`) vai no resultado da entrega (`draft`,
mantido no diário da fila) e, quando não é `returned`, num aviso do log; o texto, nunca. O envio
pelo plugin em modo `user` não passa pelo composer e não devolve o guardado (medido na prova
isolada: `plugin_accepted`, marca ainda presente), então com algo guardado o escritor usa o modo
`fill`. O Python (`_esvaziar_composer_claude`) continua apagando: é a reserva.

## Contas Codex adicionais têm origem própria

(`app/codex_contas*.py`, 10/09/2026): a padrão usa
  o `CODEX_HOME` atual; cada secundária tem outro `CODEX_HOME` e herda seletivamente a configuração,
  recursos e plugins da padrão. O fluxo legado de login único — "Login do ChatGPT (Codex) é UM login pra três CLIs",
  hoje em `superado.md` — está superado para cadastro Codex: o novo login por conta usa o protocolo nativo do Codex e não
  propaga novas credenciais para Pi/OMP. OAuth/API key são apenas métodos (`auth_method`), nunca a
  chave da conta: a identidade é `credential_id=codex:<home canônica>` e a origem do rollout.
  Autenticação, histórico, sessões, caches e confiança ficam separados; hooks herdados podem ficar
  pendentes e nunca são aprovados automaticamente. Catálogo embutido é materializado dentro da
  secundária: no CLI 0.153.4, symlink do cache ou do catálogo da padrão resulta em zero plugins, e o
  catálogo reservado só instala quando está sob o `CODEX_HOME` atual. Plugins remotos instalados por
  padrão são reconhecidos pela identidade remota, mesmo quando o inventário chama o catálogo de
  `openai-curated` e o plugin usa `openai-curated-remote`. Sessão, Arquivo e retomada preservam a
  conta; não há exclusão, rotação, migração ou troca automática por cota.
  Prova real em Linux (10/09/2026): conta padrão Pro preservada e secundária Plus conectada pelo
  login nativo, com os e-mails omitidos desta documentação pública; 20/20 plugins mantiveram
  a mesma versão/estado,
  hooks aprovados somente após pedido explícito do usuário e turno mínimo em `gpt-5.6-luna`
  respondendo `OK`. O rollout nasceu em `~/.codex-google`, apareceu no Arquivo após fechar e foi
  retomado pela mesma conta. AVD e Windows continuam sem verificação.

  Abertura da adicional (12/09/2026): o log registrou 51s entre pedir a preparação e criar a
  sessão; numa abertura posterior foram 3s. Em 13/09, essa espera saiu do formulário: ele cria o
  pane e navega, enquanto `hangar-codex-tui` inicia/acompanha o preparo no terminal e só depois
  sobe o app-server e a TUI. O GET final do preparo grava a confiança da pasta no backend, depois
  que as escritas da sincronização terminaram. Preparo já em curso é compartilhado por todos os
  launchers daquela conta nos dois sentidos da corrida criação↔preparo. O teto é 180s, acima dos
  103s medidos para plugins; `error`, falha de chamada ou estouro impede a TUI de ler config
  possivelmente antiga e segura o pane até Enter, em vez de sumir com a sessão ou seguir por um
  callback descartado do backend.
  A conferência nativa dos plugins pode ser reutilizada por até 5min, com hashes de origem e
  destino e versão do CLI iguais. Falha, confiança pendente, relógio regressivo ou pedido manual
  forçado exigem nova conferência; avisos de credenciais excluídas continuam visíveis e não
  impedem o cache. Mudança nativa fora dos arquivos rastreados só aparece na próxima abertura
  depois do prazo, ou imediatamente ao reconciliar manualmente. A primeira conferência continua
  necessária; o cache não promete abertura imediata após o prazo.
  Após a alteração, duas preparações reais consecutivas da adicional levaram 2,55s e 0,51s,
  medidas do POST até `ready`, com consultas a cada 0,5s; não inclui a abertura da conversa.

  Abertura sem espera (08/10/2026): o cache de 5 min não valia na prática. A conta
  `jefferson-felizardo` ficava `partial` com `trust_pending: true` por avisos permanentes
  (`codex_account_plugin_origin_conflict` ×5, MCP com credencial excluída), e falha ou confiança
  pendente exigem nova conferência — então toda abertura esperava a reconciliação da padrão
  (etapa `principal`) e refazia a conferência nativa dos plugins. Numa abertura o pane estourou
  os 180s e a sessão não abriu. A espera saiu do lançador: a TUI sobe com a config que existe e o
  preparo termina no backend. O preparo grava com comparação do conteúdo anterior
  (`codex_arquivos.gravar`), então a escrita concorrente da TUI no `config.toml` vira
  `codex_account_changed_during_prepare` e a próxima abertura refaz, sem perder escrita calada.
  Na mesma investigação: sessão aberta na home lista o `~/.codex/hooks.json` como hook de
  projeto (19 hooks pendentes na pergunta da TUI); o lançador passa a aceitá-lo pelo caminho do
  arquivo, além das origens `user` e `plugin`.

## Painel de saúde dos harnesses

(`app/harness_saude.py` + `harness_api.py` +
  `components/settings/HarnessSettings.svelte`, aba "Harnesses" em Configurações → Servidor): uma
  linha por CLI com o que o app instalou nele (hooks do Claude, contas, login do ChatGPT, extensões
  do Pi/omp, ponte de skills, statusline do Kimi) e um botão por item que **reusa o instalador que já
  existe** (`hook_installer.ensure_*`, `skill_bridge.rebuild`, `contas.reconciliar`,
  `oauth_codex.propagar`, o symlink das `scripts/pi/*.ts`). A checagem é só leitura; o texto vai como
  `codigo`+`params` e o front traduz (`harness_<codigo>`). O item **Credenciais** cruza o store de
  cada CLI (auth.json+models.json do Pi, `auth_credentials` do omp, `providers` do Kimi,
  `model_providers`+login do Codex) com o que o app conhece (engines.json + cofre OAuth), no nome
  que AQUELE harness usa (`provedor_embutido_do_pi` pra Pi/omp, o nome do motor pros outros);
  o card Codex tem uma seção própria de integração nativa e não oferece a ponte antiga de skills.
  "Sincronizar" reusa o `agentes_sync` e, no omp, grava a chave no mesmo SQLite do login. O Codex
  continua guardando só o nome da variável, e o resultado diz qual exportar. `instalado` é "binário no PATH OU pasta de
  config existe" porque o backend roda como serviço com PATH curto — só o binário dava "Kimi não
  instalado" com o `~/.kimi-code` cheio.

## Runtime por conta não vira atalho

(`contas._RUNTIME_DA_CONTA`, 04/09/2026): `telemetry/`,
  `feedback/`, `image-cache/`, `.last-update-result.json` (o Claude Code regrava por config dir com
  tmp+rename, que troca o symlink por arquivo real) e `.hangar-models.json` (cache do picker, por
  config dir). Ligados, cada `--prep` achava a "deriva" de novo, gavetava e disparava o toast
  "CONTA" — a gaveta desta máquina chegou a `telemetry.3`. A reconciliação desfaz o atalho antigo
  desses nomes, senão a conta seguia gravando o runtime dela dentro do `~/.claude`.

## Statusline por sidecar, não pelo pane

(`app/statusline.py` + `scripts/omniroute-statusline.js`
  + `scripts/pi/rich-status-line.ts` + `~/.kimi-code/statusline.js`): a linha que o app mostra
  (modelo, contexto, ⚡5h/📅7d, custo)
  **não** sai do transcript — quem a calcula é o agente, e o app só via o texto **já renderizado no
  terminal**, cortado na largura da janela. Medido 2026-07-30 num pane de 99 colunas: o Pi chama
  `truncateToWidth` e a linha morre em `cache…` (somem contexto, cota e custo); o Claude quebra em
  várias linhas, mas quando a quebra cai em cima do par de contexto ele vira `💬 769k/238 770k…`.
  Nos dois casos o painel dizia "medição indisponível" **por causa do tamanho do terminal**.
  Contrato: quem RENDERIZA publica a linha inteira (sem ANSI) em
  `<config>/.hangar-status/<stem>.json` = `{"line", "ts"}` — mesma chave dos outros
  marcadores (o stem do `.jsonl`) — e `statusline.read()` a prefere ao pane, caindo nele quando não
  há sidecar (sessão sem instrumentação **nunca** pode ficar sem linha nenhuma). Três detalhes que
  já custaram bug: (1) o tmp do `tmp+rename` leva o **pid**, porque o script do Claude roda a cada
  render e duas invocações da mesma sessão se sobrepõem (nome fixo → `rename` promovendo bytes
  entrelaçados, o mesmo furo que `hangar_panel_common.py` já corrigiu); (2) `read()` exige **dict** —
  JSON válido do tipo errado (`null`, lista) não levanta `ValueError` e o `.get()` derrubava a
  resolução de estado de TODAS as sessões em `list_with_state`; (3) o publicador do Pi vive na
  extensão porque a linha completa só existe dentro do processo dele — logo, **sessão Pi já aberta
  só passa a publicar depois de `/reload`** (o Pi carrega extensão na largada), enquanto o lado
  Claude vale na hora, por ser script executado a cada render. O publicador do **Kimi Code**
  (`~/.kimi-code/statusline.js`, fora do repo porque o `tui.toml` aponta pra lá) segue o lado
  Claude: script a cada render, sidecar em `~/.claude/.hangar-status/<sessionId>.json` —
  a chave é o `sessionId` do stdin, o mesmo que `session_key()` extrai do `wire.jsonl`. A linha
  dele replica os marcadores do Claude (`🤖 K3 (high✦)`, `📁 dir [branch*]`, `⚡5h`, `📅7d`,
  `🕐 HH:MM ⏱`) com duas diferenças de formato: o contexto vem como par **rotulado e sozinho**
  (`💬 ctx 480k/1M` — o stdin do Kimi não traz in/out do turno, então a regra dos "≥2 pares" do
  parser/`sse._status_sig` tem exceção pro rótulo `ctx`, a mesma do Pi) e **não há 💵** (Kimi é
  assinatura de valor fixo, mesmo motivo do Claude em motor). O ⏱ dele é a idade do
  `wire.jsonl` (birthtime), não duração de API como no Claude.

## Estado da sessão Claude pelo registro nativo (14/09/2026)

O Claude Code 2.1.271 publica, sem flag e por conta própria, um registro por processo vivo em
`<config>/sessions/<pid>.json` (`pid`, `sessionId`, `cwd`, `name`, `status`, `statusUpdatedAt`,
`messagingSocketPath`), apagado na saída normal. É a infraestrutura do `ListAgents`/`SendMessage`
entre sessões locais; as contas (`~/.claude-<nome>/sessions`) são symlink pro principal. O
`status` é a TUI dizendo o próprio estado: `busy` = turno rodando ou subagente delegado;
`waiting` = pedido de permissão, AskUserQuestion, recado de par segurado ou diálogo aberto
(`/model`, `/config`); `idle` = o resto. Medido numa sessão descartável em modo `default`:

| situação | registro | marcador do hook |
|---|---|---|
| pedido de permissão do Bash | `waiting` em 3,0 s | `working` (Notification ainda não tinha vindo) |
| Esc no pedido de permissão | `idle` na hora | `working` preso (Esc não dispara Stop) |
| AskUserQuestion na tela | `waiting` | `awaiting_input` |
| turno escrevendo | `busy` | `working` |
| `/model` aberto | `waiting` | nada |
| `kill-session` | arquivo removido | fica |

Contrato em `hook_state.py`: o registro vence o marcador enquanto `pid_vivo(pid)`; sem arquivo,
pid morto ou status desconhecido, vale o marcador e depois o pane, como antes. `waiting` vira
`awaiting_input`, e o pane continua dono da pergunta e das opções (a lista raspa quem está
`awaiting`) e do rebaixamento quando não há menu (`demote_awaiting`, só em memória — o arquivo é
do Claude e nunca é escrito por nós). No Rust (parte 4, Task 2) o rebaixamento que a lista decide
vale também num mapa da própria ponte (`state/demote.rs`), lido pela lista e pelo `Monitor` e
desfeito quando o `statusUpdatedAt` do registro muda; o aviso ao Python continua. Marcador de hook não gera transição enquanto o registro
manda pela mesma sessão, senão o drain e o push disparariam duas vezes pelo mesmo evento.

Em 19/09/2026, a reprodução com registro `idle` seguido de JSON parcial, status desconhecido
ou arquivo ausente mostrou que o cache anterior continuava vencendo o marcador `working`.
A leitura inválida agora remove essa entrada e notifica a mudança para o fallback; o próximo
registro válido volta a ser usado. Os testes cobrem também essa recuperação.

Na mesma investigação, o encerramento Windows passou a rodar fora do event loop nas rotas
assíncronas de Claude/Codex sem terminal. Falha do `taskkill` ou PID ainda vivo impede abrir o
substituto; a recarga mantém a sessão em memória e o encerramento/troca restaura seu sidecar.

O socket de mensagens (`/run/user/<uid>/cc-socks/<pid>.sock`, JSON por linha, `auth` com o
`peerToken` de `sessions/<pid>.<sha>.key`) foi medido no mesmo dia e ficou de fora: embrulha tudo
como "mensagem de outra sessão" (`isMeta`, origem `peer`), não roda comando de barra e é o mesmo
canal que o `SendMessage` nativo já usa.

Os function hooks também tinham ficado de fora nesse dia (API em acesso antecipado). Em 18/09/2026
entraram como caminho OPCIONAL por cima do tmux — ver a entrada seguinte; a decisão antiga está em
[superado.md](superado.md#function-hooks-fora-do-hangar). O interruptor é o mesmo
`claude_function_hooks` da tela de Harnesses
([plataforma.md](plataforma.md#function-hooks-configuração-do-servidor-e-por-isso-o-relançamento-relê)):
ligado, a sessão nova recebe a variável E o plugin `plugins/hangar`.

## Function hooks: o plugin `plugins/hangar` é um plus por cima do tmux (18/09/2026)

Claude Code 2.1.277, `CLAUDE_CODE_ENABLE_FUNCTION_HOOKS=1`, plugin carregado por `--plugin-dir`.
O contrato vem de `/plugin-types` (gerado por versão); o que está aqui é só o que foi MEDIDO e o
contrato não diz. Ponte: `app/plugin_bridge.py`, rotas `/api/plugin/*`, credencial = HMAC por
sessão (o bearer do app nunca entra no pane). Fallback é por AUSÊNCIA: sem long-poll vivo da
sessão, tudo segue pela tecla; interruptor desligado ou CLI sem `--plugin-dir` (sonda `claude
--help`, cache de 10 min) não põe env nem flag, e o comportamento é byte a byte o de antes.

**Regras do engine que o validador impõe** (`claude plugin validate`): um módulo por plugin
(`hooks.json` recusa o segundo), dois arquivos não hookam o mesmo evento sem matcher, e `$` não
atravessa `import` — cada arquivo lê o ambiente e chama `$.http.fetch` sozinho.

| peça | o que foi medido |
|---|---|
| envio (`input.ts`) | `$.prompt.submit` embrulha a mensagem numa moldura em inglês que nenhum hook tira (`next() passed an argument with an origin other than the engine set`). `$.prompt.fill` + UM Enter por tmux não tem moldura: 0,058 s numa linha e 0,120 s em 2 KB, contra 0,401 s e 0,680 s do `send-keys`. O Enter só sai com o rascunho confirmado, e a entrega só vale com o composer vazio depois (`_submeteu`); qualquer falha limpa o composer e devolve para a tecla |
| long-poll | 25 s segurados não gastam o orçamento de 10 s do hook: o relógio para enquanto um `$` ou o `next(e)` está em voo. `$.clock.sleep` é o único que gasta |
| token | sorteado em memória, todo restart do backend deixava a sessão viva em 403 para sempre. É HMAC do segredo do servidor + nome: refaz igual |
| `AskUserQuestion` (`ask.ts`) | hook de `tool.call`; os argumentos vêm direto em `e` (`e.questions`), não em `e.input`. `next(e)` abre o diálogo do terminal e corre contra o app: devolver `{ result: { questions, answers } }` antes fecha o diálogo. Resposta pelo app em 16 ms, inclusive com o painel de terminal aberto (onde a tecla responde 409). O resultado gravado tem a mesma forma do respondido no terminal. Backend reiniciado no meio: o hook volta a bater e a pergunta segue respondível |
| hook que lança | o engine pula o hook, escreve uma linha (`hangar: tool.call hook skipped: threw …`) e o diálogo abre normal |
| `classic.*` | Stop, Notification e PermissionRequest NÃO são entregues a plugin de `--plugin-dir`. Registrar não dá erro; o hook só nunca roda. `classic.PreToolUse` chega (2.1.291), mas com o envelope do `tool.call` (`tool`, argumentos, `tool_use_id`), sem os campos do hook clássico. Fim de turno é `turn.complete` (traz `reason`: answer/aborted/refusal/error) |
| permissão (`perm.ts`) | só `tool.check` alcança o pedido, e ele roda ANTES do diálogo: enquanto o hook segura o `ask`, o terminal não mostra nada (12 s segurados = 12 s sem diálogo). Aprovar pelo app: 8–11 ms, sem tecla. Por isso o backend só manda segurar com SSE do app aberto E nenhum cliente tmux preso, reperguntado a cada 5 s: prender um terminal no meio devolveu o diálogo a ele em 4 s, e o card do app trocou sozinho para o menu real. `AskUserQuestion`, `ExitPlanMode` e `EnterPlanMode` ficam FORA: o `ask` deles é o próprio diálogo, e segurá-lo escondeu a pergunta do terminal e do `ask.ts` (a resposta do app esperou 5 s por um aviso que não vinha). Modo auto (06/10/2026, Claude Code 2.1.291): o `next(e)` do `tool.check` devolve `ask` ("This command requires approval") também no modo auto, porque o classificador só decide DEPOIS do hook. Solto, o comando rodou sem diálogo em ~1 s; segurado, virou cartão "Escolha uma opção" no app para um subagente em background, sem nada no terminal, até alguém clicar. O plugin não enxerga o modo atual: o `classic.PreToolUse` traz só o envelope do `tool.call` (sem `permission_mode`) e o `permissionMode` do `$.config.list()` é o padrão das configurações ("default" com a sessão em auto). Por isso quem decide é o backend, pelo rodapé do pane (`permission_mode.ler_modo`) |
| estado (`state.ts`) | aviso do plugin ACORDA o `StateMonitor`: `awaiting_input` em 90 ms (o pane viu o menu 0,9 s depois), `dead` 41 ms depois do `kill-session`. `session.end` não dispara em `kill -9` e dispara em `/clear`, então quem declara a morte continua sendo o tmux. A âncora `working`/`idle` do plugin NÃO chega à tela: o marcador de hook logo abaixo dela vence — e o registro nativo (entrada anterior) já cobre esse estado |
| sugestão (`suggest.ts`) | `prompt.suggest` com `origin.kind = suggestion` traz a frase cinza do composer. Não vem todo turno (só quando o modelo consegue inferir o próximo pedido) e não há evento de descarte: quem apaga é o começo do turno seguinte |
| steer | o marcador real é `ctrl+x ctrl+s to send now`. No Claude o acorde INTERROMPE o turno em curso (`Interrupted · What should Claude do instead?`, o comando rodando morre); no Kimi o `ctrl-s` injeta sem parar nada |

O `capture-pane` a 0,75 s não foi reduzido: menu de permissão fora da regra acima, `/model`,
diálogo de confiança e morte continuam sendo do pane.

**A ponte só atende a conversa que o Hangar acompanha (01/10/2026).** `/whoami` e `/pull` recebem
`session_id` (`$.session.id()`, relido a cada poll) e só aceitam quando ele é igual ao uuid do
transcript que o `registry.resolve_tracked` dá como certo (`tracked=True`); diferente, desconhecido
ou ausente → `/whoami` responde `{"sessao": None}` e `/pull` responde 409, que o plugin trata como
perder o dono (larga a ponte, tenta de novo em 30 s). A regra real: a entrega pelo plugin vai para a
conversa que o Hangar está acompanhando na sessão (a mesma do chat e das teclas); um segundo `claude`
que não é essa conversa é recusado. Por quê: pane, ambiente herdado e pid do psmux
valem para QUALQUER `claude` aberto na sessão — um split herda `HANGAR_PLUGIN_*` do tmux — e, sem o
dono batendo (backend reiniciado), o segundo processo tomava a fila. Não é "o primeiro `claude`
sempre ganha": `tracked_session_id` segue `tmux.pane_pid` → `agentpane.resolve_target`, que prefere
o pane do agente ATIVO. Medido em `cx-uuid2`: split DESTACADO (`-d`) rodando `claude` + backend
reiniciado → o split recebeu 409 e a mensagem entrou no original pelo plugin; split ATIVO com
`claude` digitado pelo wrapper (`--session-id` próprio) → o `RESOLVE` trocou para a conversa do split
em ~3 s, o original passou a receber 409, a mensagem do `/input` entrou no split pelo plugin e o
`/history` mostrou essa mesma conversa — chat, teclas e plugin concordam. Antes, com `cx-uuid`: o
split recebeu 409 `uuid-diferente`, a mensagem foi para o original pelo plugin; depois
do `/clear` o Claude grava o jsonl novo na hora, o marcador do `state_hook` virou o vínculo em ~1 s e
a mensagem seguinte entrou uma vez só na conversa nova, ainda pelo plugin. Plugin antigo (sem
`session_id`) fica na tecla até a sessão reabrir. No Windows não há marcador (`/proc`): o vínculo é o
`--session-id` do cmdline e, depois de um `/clear`, o jsonl mais novo da pasta — com outra sessão no
mesmo cwd ele não segue o `/clear`, e aquela sessão fica na tecla até reabrir. O cwd dessa busca
sai do pane do agente (o mesmo do `list()`), não do pane ativo. `/whoami` com `{"sessao": None}`
é repetido em 2, 10 e 30 s e para: o marcador do `state_hook` pode chegar depois da largada.

### Mods no 2.1.287 medidos (01/10/2026)

Claude Code 2.1.287 (G), mod descartável `hangar-exp` (hooks `session.start`, `turn.start`,
`turn.step`, `turn.complete`, `classic.Notification`) avisando um ouvinte local; sessão de teste
com terminal criada por `hangar-send --new`, conta `~/.claude-200-01`, que já nasce com
`--plugin-dir plugins/hangar` (outro nome, convive sem conflito).

| | o que foi medido |
|---|---|
| A — pasta de skills | link `~/.claude/skills/hangar-exp` → `claude plugin list` mostra `hangar-exp@skills-dir`, `Scope: user`, `Status: ✔ loaded`. Na sessão o mod rodou de verdade: `session.start` chegou do pane `%15` sem flag nenhuma para ele |
| B — pasta de skills + `--plugin-dir` do mesmo nome | carrega UM só: `hangar-exp@inline` `✔ loaded`, e a cópia da pasta de skills sai com erro registrado — `✘ Not loaded — the name "hangar-exp" is already taken by a session-only plugin (--plugin-dir / --plugin-url), which takes precedence`. No `--json` ela vem com `enabled: false` e `errors: [...]` (`generic-error`). Não há carga dupla. O erro só foi visto no `plugin list`/`--json`; não foi conferido se aparece no terminal de uma sessão interativa |
| B2 — outra conta | `CLAUDE_CONFIG_DIR=~/.claude-claude-200-2` também lista `hangar-exp@skills-dir` `✔ loaded` (path `~/.claude-claude-200-2/skills/hangar-exp`): a `skills` das contas é link para `~/.claude/skills` |
| C1 — `submit({asUser})` parado | a promessa resolve em 0,63 s, logo depois dos hooks de `UserPromptSubmit`, antes do `turn.start`. No jsonl é `type: "user"` comum, sem `isMeta` e sem moldura: `"message":{"role":"user","content":"EXP-PARADO: responda só OK"},"origin":{"kind":"plugin","name":"hangar-exp","asUser":true},"promptSource":"system","turnOrigin":"system","queuePriority":"later"`. O digitado vem `"origin":{"kind":"human"},"promptSource":"typed"`. O terminal mostra `› Prompt from the hangar-exp plugin` acima do texto; o `/history` do Hangar mostra a bolha de usuário normal |
| C2 — `submit({asUser})` com turno rodando | entra na fila na hora (`queue-operation enqueue` às 21:42:41,822 UTC) mas a promessa só resolve 10,07 s depois, quando o turno em curso acabou e o texto abriu o PRÓPRIO turno: fim do turno 2 (marcador `idle`) 21:42:50,577 → entrada `user` 51,122 → promessa 51,887 → `turn.start` 51,927. Não injeta no meio do turno |
| C3 — hooks de configuração | `UserPromptSubmit` dispara para o texto do mod (anexo `hook_additional_context` de `UserPromptSubmit` logo após as duas entradas `user` do mod) e o marcador do `state_hook` vai a `working` (21:42:51,250, 0,67 s depois do `idle` do turno anterior) |
| D — `classic.Notification` | não chega a plugin carregado pela pasta de skills: o `Notification` dos settings disparou (marcador `awaiting_input` 60,3 s após o `idle`) e o hook do mod não rodou em 182 s parado. Mesmo resultado do `--plugin-dir` em 18/09 |
| E — primeiro texto × sidecar de prévia | `turn.step` vê o primeiro trecho de texto antes da primeira escrita de `.hangar-preview/<uuid>.json`: +112 ms, +131 ms e +39 ms nos três turnos medidos |
| F — `$.session.usage()` no `turn.complete` | `{startedAt, context:{tokens,window,percent}, rateLimits:[{kind:"five_hour"/"seven_day", percentUsed, resetsAt}], cost:{usd}}`. Bate com o sidecar da statusline do mesmo instante (`84918` tokens × `85k`; `0,887` × `$0.89`; 5h 84 %, 7d 74 % iguais). Não traz modelo nem esforço; o sidecar traz (`model`, `effort`) |

Fora do mod: pelo `/input` do app, `!echo oi` chegou ao modelo como texto (`promptSource:"typed"`,
o modelo rodou o Bash sozinho), não como modo bash; e `@README.md resuma…` enviado com o turno
rodando foi absorvido nele (`queue-operation remove`, `reason: absorbed_mid_turn`).

### Faixa dos mods: ordem na cadeia medida (03/10/2026)

Claude Code 2.1.289 (Linux) e 2.1.288 (Windows), mod descartável que hooka `ui.render`
`{ component: 'AbovePrompt' }`, grava o que `await next(e)` devolve e devolve igual; ao lado, o
mod de progresso do pmedico, que responde a faixa com a própria árvore e NÃO chama `next`
enquanto tem barra.

| | o que foi medido |
|---|---|
| sonda por `--plugin-dir`, pmedico do marketplace | a sonda recebeu a árvore inteira da barra (`Box`/`Text`/`Raster`, `surface: "terminal"`, `bodyColumns` = largura do pane): ela fica por fora |
| sonda pela pasta de skills, pmedico do marketplace | só `{"type":"engine"}` antes da barra; com a barra, o hook nem roda: a pasta de skills fica por dentro do marketplace |
| plugin do Hangar pela pasta de skills, pmedico por `CLAUDE_CODE_PLUGIN_DIRS` | o backend recebeu só `above: null`: carga de sessão fica por fora da pasta de skills |
| frequência | o hook roda a cada redesenho; com relógio na faixa, uma vez por segundo (36 vezes em ~30 s). O `ui.ts` junta os quadros em 500 ms e só envia quando a árvore muda |

A API não tem prioridade nem ordem configurável (`Tier`: `prepend`, `user`, `append`, `builtin`,
`core`; dentro de `user`, a ordem de carga). Por isso a sessão do Hangar leva `--plugin-dir`
sempre: o par com a pasta de skills já estava medido (B acima), carrega um só e sem carga dupla.

**Sessão aberta pelo terminal (04/10/2026, Claude Code 2.1.289, Linux).** O wrapper do shell
não passava `--plugin-dir`, e a sessão aberta digitando `claude` caía no caso da pasta de
skills. Medido com duas sessões vivas na mesma máquina, as duas com a barra de progresso de um
plugin do marketplace desenhada no pane: a criada pelo app (com `--plugin-dir`) tinha a árvore da barra no
`plugin_ui` do SSE; a aberta pelo wrapper tinha `above: null`. Com o wrapper lendo
`~/.hangar/plugin-dir`, uma sessão aberta por ele fora do tmux nasceu com a flag e o
`plugin_ui` trouxe a barra. O wrapper não sabe onde o repositório mora e não sonda o CLI: quem
grava o arquivo é o backend, na subida e quando o interruptor dos mods muda, e o apaga quando
`raizes_dos_plugins()` volta vazia (flag desconhecida mata a sessão ao nascer). Modo `-p` fica
sem a flag.

**Pane baixo corta a faixa no próprio terminal.** Quem corta é o Claude Code, pela altura do
pane. Medido na mesma sessão, com 150 colunas e a barra de três linhas: com 16 linhas sobra a
primeira e `↓ 2 more`; com 12, nenhuma. O painel de terminal do app é um `tmux attach` na mesma
sessão, então painel baixo cai nesse corte. Não tem relação com `--plugin-dir`.

Sem terminal o caminho é outro e não depende de ordem: o backend entra como superfície remota
(`control_request` `ui_attach`, depois `ui_render` do `AbovePrompt`) e o CLI avisa a mudança
com `system`/`ui_invalidate`. Medido num `claude -p` stream-json: a resposta traz a árvore da
superfície pedida (`desktop`/`mobile`: a barra vem como `Svg`, não `Raster`).

Feito na fase 2 dos mods, toda no `hangar-server`: a sessão Claude sem terminal atendida pelo Rust
liga a superfície `desktop` depois do `initialize` (ou ao religar ao cano), com `client_id`
`hangar`, viewport 120x40 em tela cheia e `answers: ["ui_copy"]`. Os pedidos `ui_*` saem fora do
diário da fila e por um canal próprio e pequeno, que dá prioridade à conversa: uma rajada de
efêmeros não atrasa mensagem nem permissão, e o efêmero que não cabe é descartado, porque a
superfície tem prazo e se refaz. O `plugin_ui` sai pelo hub dos aparelhos com `shown_id`,
`columns` (o `bodyColumns` de cada lugar) e `source: "surface"`, e o do Python é ignorado nessas
sessões. Clique, troca de aba e digitação vão por `ui_press`, `ui_pane_show` e `ui_input`, com uma
nova tentativa quando o `handle` vence; aviso e cópia chegam pelo canal (`ui_toast`, `ui_copy`), e
a cópia só volta ao aparelho de quem clicou quando vem do mod do botão.

Prazos: cada pedido de app (press, show, close, input e o redesenho que ele dispara) tem 3 s, o
pior caminho soma 6 s e o teto no ator é 7 s; a rota do servidor mede desde a entrada e trabalha
com 7,5 s, abaixo do corte de 8 s dos apps. O prazo da rota vai junto com o pedido até a
superfície (o do ator é o menor entre ele e os 7 s): a espera pela vez da sessão e a guarda já
gastaram parte dele. O ator descarta, sem executar, o pedido que venceu na caixa, e a superfície
só manda `ui_press`, `ui_input`, `ui_pane_show` ou `ui_close`, inclusive na nova tentativa depois
de um redesenho, quando ainda restam os 3 s do prazo do pedido; com menos, responde
`erro_mod_clique_sem_resposta` sem chamar o mod. Assim a ação não sai depois de a rota ter
desistido. O que fica é o mod lento: um `onPress` que leva mais que os 3 s responde sem resposta
com o clique já rodando. Os desenhos de fundo continuam com 10 s.

O `plugin_ui` leva a vista inteira dos mods (a árvore grande da vitrine tem ~398 KB) e, no hub dos
aparelhos, vale só o mais novo, em todas as sessões, com e sem terminal: o canal do hub leva um
marcador de versão, e cada conexão lê a vista do retrato só quando pode escrever, como o
`band_pump` do Python. Aparelho lento pula as vistas intermediárias em vez de guardá-las (antes,
até 1024 quadros por hub e 64 por conexão); os outros eventos seguem todos, na ordem, e a faixa
sai no lugar do último marcador dela. O evento e o formato que os apps recebem não mudaram.

O `claude -p` sobe com o plugin do Hangar (`--plugin-dir`) e as variáveis da ponte
(`HANGAR_PLUGIN_URL`, `HANGAR_PLUGIN_TOKEN`), exceção explícita à regra de não acrescentar nada ao
Python (S7, no lançador do `adapter.py`): a URL que um mod abre num clique do app (`xdg-open`) vai
ao aparelho de quem clicou pelo `press-start` e pelo `opened`, que o Rust atende; nesse processo o
aviso e a cópia não passam pela ponte, para não chegarem em dobro. Com os mods desligados, o
filho nunca herda `HANGAR_PLUGIN_*` do ambiente do backend.

### Ponte local com bind específico (issue #80)

A URL da ponte, no ambiente da sessão e no arquivo de descoberta, é
`http://127.0.0.1:<porta pública>/api/plugin`. O bind público em um endereço específico não
atende esse destino. Trocar a URL pelo endereço de rede também não basta: `whoami` e
`submitted` exigem origem loopback, e processos vivos conservam o ambiente de lançamento.

No modo Rust, `plugin_listener` confere o endereço e a porta efetivos do listener público e,
quando necessário, abre `127.0.0.1` na mesma porta. `0.0.0.0` e `127.0.0.1` já cobrem o destino;
outros IPv4, inclusive outro endereço do bloco loopback, não. Em IPv6, `IPV6_V6ONLY` vem do
socket real: wildcard dual stack não ganha outro bind; v6-only e endereços específicos precisam
garantir também o IPv4 anunciado. Não se altera essa opção do socket público.

Essa entrada local só monta as rotas da ponte, compartilhadas com a entrada pública. Os
handlers Rust usam o mesmo estado e as rotas ainda atendidas pelo Python seguem ao mesmo
upstream, preservando corpo, método e origem. Token da sessão e chave de descoberta continuam
obrigatórios; loopback não concede identidade de dono e o segredo interno não passa pelo proxy.
Outros caminhos recebem 404, sem expor a API inteira nem as pontes privadas.

Os binds terminam antes da saúde pública. Conflito de porta impede a partida, em vez de trocar a
porta contratada ou aceitar outro processo como substituto. O serviço adicional participa da
mesma seleção de serviço e parada; fechar o cano do pai ou falhar um listener encerra o conjunto.
URL, sidecar, IPC e formato de evento não mudam.

O nome HTTPS/MagicDNS usado pelo app é independente dessa conexão local. `tailscale serve`
continua encaminhando ao backend como antes, e bind direto na interface Tailscale segue a mesma
política de endereço específico. O adicional não torna a API inteira disponível em loopback
numa configuração que antes não a atendia. Sem Rust, a reserva Python mantém o limite antigo.

Prova de 07/10/2026, protocolo interno 36, com Claude Code real no Windows e dois clientes do
app nativo acessando por HTTPS/MagicDNS do Tailscale, sem turnos de modelo:

- Na base, bind IPv4 específico atendia a API pública, mas recusava loopback na mesma porta. A
  faixa existia no terminal e faltava no app; o clique sem terminal não entregava URL ao cliente.
- Com a entrada local, a faixa apareceu no app e o clique do terminal abriu a URL somente no
  primeiro cliente; o clique sem terminal abriu somente no segundo. Cada cliente recebeu uma
  abertura. Depois de reiniciar o servidor mantendo as sessões, outro clique do primeiro cliente
  somou uma abertura somente nele.
- Token inválido recebeu 403. API pública geral, saúde e pontes privadas receberam 404 pelo
  adicional. A descoberta com a chave do sidecar e o UUID real da conversa devolveu a sessão e
  seu token, sem trocar o endereço anunciado.
- `0.0.0.0` e `127.0.0.1` mantiveram a API pública no loopback, com 401 sem credencial, sem bind
  duplicado. `::1` e `::` no Windows abriram também a entrada IPv4 restrita; no wildcard IPv6
  dessa máquina o socket era v6-only. Dual stack tem regressão de socket, mas não foi executada
  nessa prova; não se presume seu resultado a partir do Windows.

Os builds Linux e Windows foram executados. No fechamento Linux, com Rust 1.98.1 e ambiente
isolado, passaram as regressões da ponte, incluindo os sockets dual stack e v6-only. O workspace
teve 1.169 testes aprovados e uma falha em `failed_open_leaves_no_entry_and_frees_the_lease`
(`cano_read` em vez de `cano_connect`); o teste passou na repetição isolada. Ele libera uma porta
efêmera antes da conexão, deixando uma janela de reutilização. Não houve alteração desse teste.
A suíte do backend teve 8.459 aprovados e 76 pulados; a contenção/runtime teve 113 aprovados e
um pulado; o contrato Python com o cano Rust teve 23 aprovados. A fixture HTTP nova foi corrigida
para não depender da feature JSON do Axum, que o projeto não habilita.

A verificação Windows posterior rodou em checkout isolado, sem alterar a instalação em uso.
O workspace teve 1.066 aprovados e duas falhas em testes que lançam `python3`
(`peek_keeps_old_writer_tcp` e `two_processes_one_lease`). Ambos passaram ao repetir com
`python3.exe` da venv de teste no PATH, em vez do atalho da Microsoft Store. As regressões da
ponte passaram, incluindo os sockets IPv6; os seis arquivos de contenção/runtime tiveram
106 aprovados e oito pulados.

Esses passos Linux e Windows rodaram sobre o diff da worktree, sem commit nem registro de
aprovação de árvore. O registro oficial continua exigido antes da publicação.
A fixture da prova visual isolou instalação/atualização automática na subida, igualmente na
base e na correção, para não substituir o processo de prova por outro ciclo do backend;
bootstrap de sessões, hooks, lifespan e proteção dos escritores continuaram reais.

### Guarda e erros da interface dos mods

Antes de cada operação de mod, o Rust pergunta ao Python se a troca de agente está em curso
(`/internal/sessions/{name}/transfer`, contrato interno 28) e devolve a recusa dele
(`session_transfer_busy`); sem resposta, recusa com `erro_mod_guarda_indisponivel`. Essa guarda
reaproveita a proteção que já existe no Python, em vez de portar a leitura do registro para o
Rust.

Códigos de erro novos ou revistos, todos com texto em pt e en, no `ERROS` do core e nas listas de
teste de recusa do web e do nativo: `erro_mod_fechar_recusado`, `erro_mod_painel_inexistente`
(o show de um painel que não está mais na frente, em vez da frase de botão, que seria falsa para
uma aba), `erro_mod_guarda_indisponivel`, e as frases genéricas de `erro_mod_botao_inexistente`,
`erro_mod_clique_sem_resposta` e `erro_mod_sem_digitacao` (esta vale também para a sessão que
não atende como superfície). Nos dois apps, falha de show ou de input com código conhecido usa a
frase do código mesmo num 5xx; só sem código cai na frase genérica. No celular, a faixa dos mods
ganhou o botão "Ocultar", que desliga os mods na hora (a mesma preferência do menu).

Limites conhecidos:

- a guarda é perguntada uma vez, na entrada da operação, enquanto a rota do Python segura o
  ingresso até o fim; uma troca que comece entre a resposta e o `ui_*` não é vista;
- cada `change` de um campo paga uma ida ao Python (a guarda), com a falha dela virando
  `erro_mod_guarda_indisponivel`;
- o convidado não digita em campo de mod nas sessões sem terminal, e também não vê a faixa nem os painéis delas: o `/events` do convidado vai ao Python, que não tem a interface dos mods das sessões que o Rust atende (servir o `plugin_ui` ao convidado pelo hub fica para outra decisão);
- sem o servidor Rust, o `opened` com bind específico continua sendo limite da ponte; com o
  Rust ativo, a [entrada local](#ponte-local-com-bind-específico-issue-80) mantém a URL anunciada;
- a sessão sem terminal renomeada continua com o mesmo `claude -p`, que manda à ponte o nome com que nasceu (`CP_SESSION_NAME`) e o token desse nome. O renomear fecha e reabre a sessão no Rust no mesmo processo (chave durável e cano), e a reabertura herda o nome de nascimento; com isso `press-start` e `opened` acham a sessão. Se o `hangar-server` reiniciar depois do renomear, ele não conhece o nome antigo, e a URL de um clique do app abre na máquina do servidor até o processo ser relançado;
- o token da ponte é o HMAC só do nome, como no Python: dois processos que nasceram com o mesmo nome (uma sessão renomeada e outra criada depois com o nome antigo) têm o mesmo token, e o servidor não os distingue. Com as duas vivas no Rust, a ponte não atende nenhuma (a URL de um clique do app abre no servidor), para um processo não tomar o clique nem mandar URL ao aparelho da outra. Quando o nome antigo é hoje o de uma sessão fora do Rust (com terminal, ou atendida pelo Python), o Rust pergunta ao Python, sem cache, se a sessão existe; existindo, ou sem resposta, `press-start` e `opened` vão ao Python, que atende essa sessão, e a renomeada perde o efeito do clique do app (a URL abre no servidor) enquanto as duas viverem. Na interface, cada vida de ator tem identificador próprio, e uma sessão nova com o nome antigo não herda faixa, avisos nem clique. Fechar o limite pede um token por processo (o nome de nascimento e a chave no sidecar e no HMAC, no Python e no plugin), fora da exceção S7;
- a sessão com terminal que o Rust atende, inclusive no Windows (onde o terminal já nasce no Rust), não é superfície remota: a fonte da interface dos mods dela é o plugin do Hangar no terminal, e desde a fase 3 a ponte dela e o clique do app são do Rust (parágrafos seguintes).
- o aparelho que acompanha recebe a vista inteira a cada mudança da faixa, sem gzip (SSE): a barra de progresso de um mod, que redesenha a 1 Hz com um painel grande aberto, custa ~400 KB/s por aparelho. Cortar isso pede mudança de contrato com os apps (um evento só da faixa, ou revisão por painel com a árvore omitida quando não mudou, com anúncio de capacidade no `/events` para os apps atrasados), fora deste trabalho.

Feito na fase 3 dos mods, com terminal, no `hangar-server` (`mods/screen.rs`, `mods/click.rs`, `mods/terminal.rs`, `mods/bridge.rs`, `runtime/terminal.rs`), em Linux, macOS e Windows (psmux). O plugin do Hangar continua sendo a fonte da interface: manda a faixa e os painéis com o `bodyColumns` ao lado do `columns` (que soma as 5 colunas do `[-]` e é o que o Python usa para cortar a prévia), registra o painel já no `ui.open` colocado, informa o último painel desenhado (`shown`) e repassa `ui.scroll` e `ui.focus`. O Rust não lê a árvore do terminal: lê a tela.

- **Leitura da tela (`mods/screen.rs`).** O `capture-pane -e` vira uma grade de células com o estado absoluto do SGR (o psmux escreve `ESC[0;1;7;48;2;…m`, lido parâmetro a parâmetro). Dela saem a faixa (cheia, recolhida ou encolhida), o painel na frente e a região dele, a linha de abas, a aba ativa, a célula do `✕`, quem tem o foco (painel, faixa ou prompt) pela cor da borda e pelo inverso da faixa, e diálogo ou pesquisa de satisfação na tela. O servidor publica `shown_id` lido da linha de abas, em janelas de 300 ms, e `source: "terminal"`. A leitura é conferida contra 34 capturas reais do Claude Code (tmux e psmux), guardadas como dados de teste sem link de sessão, e-mail nem caminho pessoal.
- **Operações do driver (tmux e psmux).** Ler formatos, ler clientes, capturar a tela, mouse SGR, roda, teclas de uma lista fechada e redimensionar. O sinal de mouse ligado é `#{mouse_sgr_flag}` no tmux e `#{alternate_on}` no psmux; formato vazio nunca vale 0. Clientes de controle do próprio Hangar não contam como terminal ligado (no tmux, `list-clients -F` sem `control-mode`; no Windows, `#{session_attached}`). Todo redimensionamento devolve `window-size latest`.
- **Executor (`runtime/terminal.rs`).** As operações de mod são mensagens do mesmo executor da entrada de texto, seriais com ela e fora do diário da fila. Cada uma leva um `start_by`: a que chega à vez depois dele não age e volta recusada, para que uma tecla esquecida na caixa não saia depois de o app ouvir que o clique falhou. `Hold` reserva o pane ao clique (teto de 10 s, vence sozinho se a tarefa sumir) e `Release` solta sempre, mesmo atrasado: com a reserva, comandos e pedidos de drenagem ficam guardados e saem na ordem depois dela. Fora da reserva, a escrita da entrada também adia (`mods_focus`) quando a tela mostra o foco num painel ou na faixa do mod, porque o `Enter` da mensagem apertaria um botão; só conta o foco dentro de painel ou faixa reconhecidos, para o realce do próprio Claude Code acima do prompt nunca atrasar a mensagem da pessoa. A guarda só roda quando a entrega vai apertar tecla (o modo `Fill` do plugin e a digitação sem plugin): no modo `User` e na entrega nativa a mensagem entra sem tecla, e a guarda só custaria duas leituras do multiplexador e atraso. A entrada adiada assim não espera para sempre: com o foco num mod há mais de 10 s, contados da primeira vez que a linha o viu, e nenhum clique em curso (a limpeza que desistiu, ou a pessoa que levou o foco a um painel e saiu), o executor devolve o teclado ao prompt por `ctrl+x tab`, lendo a tela antes de cada tecla e parando num diálogo, e só então escreve. Cada passo é a tecla e uma leitura repetida até a tela mudar (até 300 ms), com o pane conferido e o tamanho lido uma vez: na VM a volta andava a 0,5 s por passo e a mensagem saía em 15,8 a 21 s. Cada linha tenta duas devoluções, a segunda 20 s depois da primeira; depois só registra. A contagem é da linha: a seguinte começa do zero. Com só a faixa na tela (nenhum painel), o `ctrl+x tab` gira dentro dela e nunca chega ao prompt (medição (i) do tmux): nenhuma tecla é mandada, e a entrada espera a pessoa. A faixa inteira é reconhecida pela âncora do mod (o começo do primeiro texto dela), que o `Mods` entrega ao elo a cada `/ui` e o elo grava numa âncora compartilhada com o executor, criada junto com ele no `open_terminal`; a recolhida e a encolhida, pela própria linha. Com o teclado emprestado ao Python, a operação de mod é recusada.
- **Clique por mouse.** O pedido inteiro cabe nos 7,5 s da fase 2, medidos desde a entrada da rota: cada ação no mod (clique, roda, tecla, redimensionamento) só sai com tempo para a confirmação do clique final (2 s) e uma folga de 300 ms, e a roda vai até 4 s. O clique lê a tela antes de cada passo, ativa a aba pelo meio do título, procura o rótulo só na região do alvo, confere que a célula não andou e que a aba continua na frente, e confirma pelo `press` do plugin, sem repetir às cegas. Botão fora da área visível é alcançado esticando a janela a 250 linhas (só sem terminal de verdade ligado) ou pela roda, acompanhando o `offset` do `ui.scroll`, nunca uma conta de linhas. Cada evento espaçado da roda rola uma linha no tmux e três no psmux (a rajada acelera sem conta previsível), então um botão a dezenas de linhas não cabe no prazo. A escolha entre o mouse e o teclado é feita antes de ativar a aba: o que a reserva por teclado precisa é um passo de até 0,2 s por parada do anel até o painel (os botões da faixa só quando estão no anel, nenhum com ela recolhida, e os sem desenho também a 0,2 s, porque o passo termina no `ui.focus` deles e não na espera da tela), o `Tab`, a espera do foco e a confirmação. Se ativar a aba já não deixa esse tempo, o clique vai direto ao teclado, que traz a aba pelo anel; senão a roda vai até o teto dela ou até a hora em que o teclado ainda cabe, e passa a ele, cujo `Tab` rola o painel sozinho até o botão. Um painel que a roda não rola nas duas direções também passa ao teclado. Se o teclado não cabe nem no começo, o mouse fica com o prazo inteiro (provas de 06/10/2026: a roda parava na linha ~20 de 200; o anel pagava 0,3 s de espera da tela por botão invisível da faixa e chegava ao painel sem prazo; com outra aba na frente, a ativação comia o tempo do teclado). Rótulo repetido na árvore do mod (botão homônimo fora da área visível ou substring de outro) não é único: vai ao teclado em vez de arriscar o botão errado. O `close` relê a aba ativa logo antes de clicar o `✕` e o clica pela célula exata. Dois cliques de mouse no mesmo pane saem com 550 ms entre eles sempre que o prazo comporta, também de um pedido para o seguinte; com o prazo curto, 350 ms bastam para um clique a mais de duas células do anterior, e o vizinho não sai sem os 550 ms. Mais perto que isso, o Claude Code os toma por duplo clique, engole o segundo e às vezes copia a palavra clicada para o buffer do multiplexador (medido no tmux em 06/10/2026: a duas células ou mais, até 250 ms o segundo some e a partir de 300 ms vale; a até uma célula, como o botão logo abaixo do título da aba, some até 450 ms e vale a partir de 500 ms). A espera entra na conta do prazo: sem tempo para ela, o clique não sai. Cada passo confere a vida da sessão que pediu: a leitura, a escrita e a espera só valem no registro dessa vida, e um clique da sessão substituída não escreve na nova.
- **Limpeza garantida.** O pedido roda numa tarefa própria, que a rota não cancela. A resposta vai ao app assim que o resultado existe; depois, com prazo próprio de 2 s, a limpeza devolve a altura da janela, volta o teclado ao prompt, desarma o alvo do foco e solta o pane, também quando o pedido foi cortado no meio. Cada item entra no registro de desfazer antes da ação que o pede. A volta ao prompt tem prazo próprio, de 0,45 s por parada do anel e mais uma, entre 2 s e 9 s: no psmux cada passo custava 0,4 s, e com doze botões na faixa e dez painéis (23 paradas) os 2 s de antes desistiam com o teclado num painel (prova na VM, 06/10/2026). A limpeza renova a reserva para cobrir esse prazo, e a volta que falha segue tentando por até 10 s com o pane ainda reservado, porque `Escape` está proibido (age no Claude, e no psmux nem devolve o foco).
- **Reserva por teclado.** Só fora da tela cheia e só nos casos medidos: rótulo repetido no mesmo painel, título de aba fora da linha e o `ctrl+x tab` que traz o painel ou o botão da faixa. Exige o prompt na tela e vazio, cada passo confirmado pelo `ui.focus` do plugin e a volta por `ctrl+x tab`. O `Enter` só sai depois de ler a tela e conferir que o último foco visto é o do alvo, no lugar pedido, e que nenhum foco mais novo o levou a outro elemento; com o alvo já focado pela primeira tecla, o `Tab` seguinte não é dado. Com diálogo na tela só valem o botão do painel já mostrado ao lado da conversa e o fechar dele. Cada passo do anel custa a tecla e uma leitura da tela, sem reler o tamanho, e a leitura depois da tecla se repete até ver a tela mudar (até 300 ms; a tecla aparece em 50 a 60 ms): no psmux cada operação é um processo, e com a conferência do pane e o tamanho relidos a cada passo um anel de doze botões na faixa custava 0,4 s por passo e estourava o prazo (prova na VM, 06/10/2026). Dentro da reserva do pane o executor confere a identidade do pane uma vez só. A espera por uma reescrita já no `ctrl+x tab` de entrada no painel é a da tela, não a do foco: nas provas ela vem no `Tab`.
- **Vigia do tamanho (`mods/terminal.rs`).** Sem terminal de verdade ligado, o Hangar mantém a janela em 144 colunas por 40 linhas. O vigia ouve os avisos de cliente e de janela do multiplexador e repõe o mínimo; se um clique está em curso, espera a vez pelo orçamento da rota mais a limpeza mais longa, porque o terminal que se desliga no meio do clique não avisa de novo. No Windows o vigia recusa (o psmux não avisa, e o Hangar não liga cliente de controle lá): o mínimo volta no preparo de cada operação de mod.
- **Elo e posse no gateway.** `TerminalLink` é a ligação da sessão com o `Mods`: leva os pedidos dos apps ao clique, lê o painel na frente e encerra o vigia ao sair de escopo. O gateway cria uma vida única por abertura (`new_life`), grava-a na entrada, e `attach_terminal` registra o processo (chave durável, pane e criação, que o renomear mantém). Antes de ligar, `unstretch` devolve a janela que um clique cortado pelo fim do servidor deixou com 250 linhas (só sem cliente ligado), e o vigia sobe depois do `attach`, para não derrubar o próprio elo. A reabertura da mesma vida só religa o elo quando o nome saiu do `Mods` (`mods.life(name)` é `None`); nome que já tem outra vida fica com ela, porque é uma sessão mais nova e viva, e tomá-lo a deixaria sem dono e sem o nome de nascimento. O `close` esquece a entrada pela vida dela.
- **Ponte com terminal (`mods/bridge.rs`).** O plugin que roda no terminal usa todas as rotas: faixa e painéis (`ui`), avisos, `press`, cópia, URL, foco da reserva por teclado e rolagem; sessão que o Rust não atende segue ao Python com o mesmo corpo. A cópia do `/ui` no formato de hoje segue ao Python, para a prévia e o campo `faixa` do `/pull`, que continuam lá; vai com o nome atual da sessão e o token dele, porque o cache do Python é pelo nome de agora, e o plugin de uma sessão renomeada continua mandando o nome de nascimento.
- **Digitação.** Com terminal não há por onde digitar no campo do mod: `input` é recusado com `no_typing`, sem reservar o pane, e o `ui.input` do app responde `erro_mod_sem_digitacao`.
- **Convidado.** O Rust não autentica convidado (convite ou usuário convidado). Por decisão do dono do projeto, e sendo a exceção à regra de não acrescentar nada ao Python, o Python recusa o convidado com 403 `erro_mod_convidado` quando o terminal da sessão é do Rust, em duas camadas. A rápida é a dependência do `plugin_press`, que lê uma fotografia da posse (`terminal_in_rust`). Ela não basta: sem slot (primeira operação depois de reiniciar o backend) ou no meio de um open/detach, a fotografia mostra o terminal fora do Rust, e o próprio clique abriria a sessão no Rust e receberia o teclado emprestado. A que vale é a recusa refeita no `_borrow_keyboard`, sob a barreira da sessão e com a posse do Rust já conferida: o handler põe a marca de convidado num `ContextVar` (`guest_admin`), o `_click` a copia até a thread do driver, o `wrap_driver` a passa ao `run_admin`, e o `_borrow_keyboard` levanta `GuestRefused` (de propósito não é `RuntimeError`, para quem trata falha de escrita como "sem resposta" não engolir a recusa), que o `plugin_press` devolve como o mesmo 403. O Rust deixou de tentar reconhecer o formato de pedido de convidado: repassa ao Python todo pedido que não é do dono, que recusa o convidado ou responde 401 ou 429 e conta a falha. Sem isso, o convidado chegaria ao `plugin_click.py`, que dirigiria o pane do executor do Rust.

Limites conhecidos da fase 3:

- a faixa inteira focada só é reconhecida com a âncora do último `/ui` do plugin: um mod que muda o primeiro texto da faixa entre um desenho e a leitura da tela (antes de o `/ui` seguinte chegar) deixa a faixa sem reconhecimento por esse intervalo, e a mensagem pode sair com o botão focado;
- a reserva por teclado tem uma janela residual: com dois eventos de foco atrasados (o do `ctrl+x tab` de entrada e o do `Tab`), o `Enter` pode sair com o foco em outro botão; as conferências (tela lida antes do `Enter` e último foco visto no alvo) a estreitam, e a limpeza do clique fica como a proteção principal;
- terminal que se desliga durante um clique, sem a janela esticada, deixa a janela abaixo do mínimo até o vigia poder agir: ele espera a vez do pane pelo pedido e pela limpeza mais longa;
- quem lê um painel de mod no próprio terminal enquanto uma mensagem do app que aperta tecla (`Fill` ou digitação) espera perde o foco para o prompt depois de 10 s; a guarda decide pelo modo previsto com os fatos da sessão, antes de ler o rascunho: com um rascunho guardado o plugin passa a `Fill`, e a entrega nativa que não escreve cai na digitação, as duas sem a guarda;
- com o foco na faixa de um mod e nenhum painel aberto, a entrada que aperta tecla espera a pessoa tirar o foco de lá (o `Escape` devolveria, mas é proibido); depois de duas devoluções que falham, também;
- a devolução do foco por `ctrl+x tab` percorre o anel: cada painel seguinte vem à frente no caminho, e a pessoa perde também a aba que via. Ela para no primeiro ponto que a leitura não reconhece como mod; com a âncora da faixa defasada (o mod mudou o primeiro texto e o `/ui` ainda não chegou), esse ponto pode ser um botão da faixa, e a entrada é escrita com o foco nele. O `Escape` devolveria sem mexer na aba, mas é proibido (age no Claude, e no psmux só o formato Win32 funciona). A medição mostra outro caminho, o clique numa célula da conversa, que devolve o teclado sem trocar de aba (tmux e psmux); ele não foi adotado porque só vale em tela cheia e uma célula da conversa pode ser link ou linha de ferramenta que abre com o clique, e escolher uma célula segura pede leitura própria da conversa;
- no Windows o vigia não roda e a contagem de clientes usa `#{session_attached}` (o psmux conta um cliente em modo controle, mas o Hangar não liga um lá);
- com o processo inteiro sem Rust, o clique de mod com terminal é o de antes (`plugin_click.py`), sem abas seguindo o terminal, sem reserva por teclado e sem controle de tamanho;
- o convidado não aciona nem vê os mods de sessão com terminal do Rust pelo app; a recusa vale por dois lados (Rust e Python) para fechar a diferença de leitura de cabeçalho entre eles.
- convidado que pede `show` ou `input` numa sessão com terminal do Rust recebe 405 do Python, que só tem as rotas `plugin/press` e `plugin/close`, em vez do 403 `erro_mod_convidado` delas: nada é acionado, mas o app mostra um erro genérico. Fechar pede as duas rotas no Python ou a recusa do convidado no Rust;
- a publicação da vista é montada fora da trava e conferida sob ela pela versão, mas entregue aos aparelhos depois de soltá-la: numa corrida entre dois `/ui` (ou um `/ui` e a leitura da tela), a vista mais velha pode chegar por último. O próximo `/ui` ou a próxima leitura da tela corrige (T11-a);
- a limpeza renova a reserva do pane no começo e de novo antes de devolver a altura, mas entre a última renovação e o `Release` a reserva ainda pode vencer alguns milissegundos antes do fim quando a operação no executor demora: a fila só entrega com o teclado já no prompt, então a mensagem não aperta botão, só pode sair um instante antes de a altura voltar (T13-a);
- dentro dos 3 s depois de um foco armado, o plugin pergunta ao backend se o alvo ainda está armado antes de derrubar um envio do composer, e só derruba com a confirmação; sem resposta em 2 s (sessão renomeada com o Python lento ou fora do ar), o envio passa, e uma letra que a pessoa digitasse no meio da reserva, com o `Enter` do clique, poderia ir ao modelo. A reserva recusa já de início com rascunho no prompt. Responder `Deferred` pelo aviso do plugin na tela foi tentado e revertido: o texto do aviso pode estar na tela por outra razão, e a linha seria digitada de novo;
- o hook de `ui.focus` do plugin espera o `focus-target` sem prazo próprio antes de seguir; numa sessão renomeada cada movimento do anel paga também a pergunta ao Python sobre o nome antigo (até 1 s). O hook tem o teto de 10 s do engine.

### Mods: painel, clique e o que acontece no aparelho (04/10/2026)

Claude Code 2.1.289 (Linux), sondas descartáveis carregadas por `--plugin-dir` ao lado do mod
`review-mr` do pmedico (MR mergeado: lê o GitLab e encerra, sem efeito).

| | o que foi medido |
|---|---|
| `ui.render` de `{ component: 'Pane' }` sem `requestId` | recebe o painel de OUTRO plugin com a árvore inteira e as props `title`, `placement` (`dock`/`inline`), `bodyColumns`, `scroll` |
| `ui.press` sem matcher | vê o clique de outro plugin: `{plugin, element (key), component, requestId, surface}`; `requestId` é `above-prompt` na faixa e o id do painel no painel |
| `ui.copy`, `ui.open`, `ui.close` | vistos com o texto copiado, o id/título/colunas do painel aberto e o `origin` de quem fechou |
| chamadas do `$` (`OpEventOf`) | `process.run` de outro plugin é interceptável por nome; responder `{ value }` sem `next` impede o `xdg-open` de rodar na máquina do terminal. `origin.plugin` diz quem chamou |
| disparar press de outro plugin | não existe método no `$`; só a superfície remota (`ui_client_press`, sessão por stream-json) ou o próprio terminal |
| clique SGR por `send-keys -l` | `ESC[<0;x;yM` + `ESC[<0;x;ym` chega ao Claude Code em tela cheia (`#{mouse_sgr_flag}` = 1): abriu o painel, copiou o link e fechou o painel pelo `✕` do engine |
| botão sem `plain` | o terminal desenha `[ rótulo ]` em volta do `label`; a busca pelo rótulo acha o texto dentro |
| `onPress` do mod | dispara `$.ui.copy`/`$.process.run` sem `await`: a cópia pode chegar ao backend depois do `pressed`. Por isso a janela de 1,5 s no plugin e a espera de 0,3 s pelo efeito no backend |
| prévia com painel ancorado | antes do corte, o SSE mandava `"text":"ok   …   │\n   …   │"` |

**psmux (WinBoat, Windows 26200, psmux 3.3.8, Claude Code 2.1.289):** o psmux não tem
`#{mouse_sgr_flag}`/`#{mouse_any_flag}`/`#{mouse_button_flag}` (vêm vazios); `#{alternate_on}` e
`#{pane_in_mode}` existem. Aberto por SSH, o Claude Code registra "fullscreen disabled: Windows over
SSH (ConPTY re-rendering) detected" e fica fora de tela cheia (`alternate_on` = 0): o clique SGR não
pressiona nada, e os bytes também não aparecem no prompt. Com `CLAUDE_CODE_NO_FLICKER=1`, mesmo sem
`"tui": "fullscreen"`, ele entra em tela cheia (`alternate_on` = 1) e o mesmo `send-keys -l` com o
par SGR pressiona o botão (o `onPress` da sonda gravou o arquivo).

O engine recusa carregar um módulo que guarda o próprio `$` numa variável (`engine = $`); só aceita
o `$` no ponto da chamada ou num closure, como nos timers. O `tsc` não pega isso, o
`claude plugin validate` pega. Um painel que o mod abre sem pedido da pessoa só é desenhado a
partir de 144 colunas (110 depois de pedido); abaixo disso não há árvore para espelhar.

### Mods: o mod vai no contrato dos apps (06/10/2026)

A `key` de um controle só é única dentro do mod que o desenhou: dois mods podem desenhar a mesma
`key` no mesmo lugar (faixa ou painel). Antes, o app mandava só o lugar e a `key`, e o servidor
recusava os dois controles, porque acionar o primeiro podia ser acionar o mod errado. Agora o app
manda também o mod (`plugin`) que leu do `press` do nó, e o servidor resolve o controle por lugar,
mod e `key`, com e sem terminal. Medido no Claude Code 2.1.292: a árvore que o terminal entrega ao
plugin do Hangar já traz `press: {plugin, handle}` em cada botão, então o clique pela tela filtra
o rótulo pelo mod como a superfície remota filtra o `handle`.

- Os apps mandam `plugin` em `plugin/press` e `plugin/input`. Fechar painel (o `✕`) tem rota
  própria, `plugin/close` com só o `site`, em vez da `key` reservada `__close__` no `press`.
  `plugin/show` não muda: o id do painel já é único.
- Ponte entre versões, nos dois sentidos. Servidor novo, app velho: sem `plugin`, o Rust e o Python
  acham o único mod com a `key` no lugar, como antes, e recusam a `key` de mais de um mod
  (`erro_mod_botao_inexistente`); `press` com `__close__` e sem `plugin` continua fechando o painel.
  App novo, servidor velho: `plugin/close` com 404/405 vira `press` com `__close__`, e `press`/`input`
  com 422 (o corpo estrito recusa o campo novo) vão de novo, uma vez, sem `plugin`.
- Controle sem `press.plugin` (ou sem `key`) é só rótulo nos apps: não há como o servidor achá-lo.
- O campo do app (`Input`) é identificado por lugar, mod e `key`, no web e no nativo. Medido no
  2.1.292: o engine recusa a faixa inteira quando dois mods desenham `Input` com a mesma `key` no
  mesmo lugar ("Input "campo" is drawn twice; each takes its own key") e desenha a dele; dois
  `Button` com a mesma `key` passam. O mod no `input` segue o do `press`, e não depende dessa recusa.
- O plugin do Hangar manda o mod no `press-start`, no `pressed` e no `focused`, e o servidor só casa
  o press, e confirma o foco do clique pelo teclado, no mod pedido. Sem o campo (plugin já carregado
  numa sessão viva antes desta versão), o casamento continua por lugar e `key`, e o foco sem mod
  numa `key` de mais de um mod no lugar recusa o clique, sem `Enter`.
- O mesmo mod com a mesma `key` duas vezes no lugar continua recusado: não há como saber qual.

### Aviso de mod medido (04/10/2026)

Claude Code 2.1.289 (Linux), plugin do Hangar e um mod de prova carregados por `--plugin-dir`; o
mod chama `$.ui.toast(texto, { timeoutMs: 15000 })` a cada 20 s, sem turno nenhum em curso.

| | o que foi medido |
|---|---|
| terminal | caixa no canto superior direito, o nome do mod na primeira linha e o texto cortado com reticências na largura da caixa |
| `ui.toast` sem matcher, no plugin de fora | recebe o aviso de OUTRO plugin: `{ text, timeoutMs }`, e `next.origin.plugin` é o nome do mod que chamou. `next(e)` deixa o terminal desenhar como antes |
| transcript e estado | nada: o aviso não gera linha no `.jsonl` nem transição de estado. Sem evento próprio ele nunca chegaria ao app |
| reconexão | a conexão nova recebe o aviso ainda vivo com `timeoutMs` igual ao que resta; o mesmo id chegou a três conexões seguidas e a tela mostrou uma caixa só |
| backend no Windows | aviso de 12 s: a conexão aberta recebeu `timeoutMs` 11999, a aberta 4 s depois recebeu o mesmo id com 7943; token errado no `POST /toast` dá 403; o evento sai sem `id:` de SSE |
| nativo | o `autohide` da notificação do gpui-component é fixo em 5 s; com ele desligado e um temporizador próprio, o aviso de 15 s ficou na tela em +1, +5, +10 e +14 s e tinha saído em +16,5 s |
| web | `--surface-raised` deixa o texto da conversa vazar pela caixa, e o degradê da navbar desce além de `--nav-h` e apaga o nome do mod: a caixa é opaca e fica acima da navbar |

O aviso preso por um painel aberto com `holdToasts` espera no terminal e sai na hora no app: o
`ui.toast` passa pelo hook quando o mod chama, não quando o terminal desenha.

### Régua com o nome da sessão (06/10/2026)

Claude Code 2.1.292 (Linux, tmux, tela cheia). Uma sessão aberta com `--name sessao-de-prova`
desenha a caixa de digitar assim, com o nome no fim da régua de cima e a de baixo pura:

```
──────────────────────────────────── sessao-de-prova ─
❯ Try "…"
──────────────────────────────────────────────────────
```

Com a régua exigindo só `─` e espaços, a tela não tinha caixa de digitar. Os mods davam diálogo
aberto em todo clique ("Há uma pergunta aberta no terminal da sessão"). No `terminal_state`, o
`composer_end` deixava de achar o par de réguas. Com o painel de agentes em foco, a análise virava
`awaiting_input` com a pergunta "Enter to view · x to stop" (o defeito do #85 de volta), e a
prévia em voo descartava o `❯` da caixa como se fosse mensagem do usuário. As capturas
`tmux-160`/`tmux-161` em `crates/hangar-server/tests/fixtures/mods_screen` e o teste
`terminal_state_session_name_in_the_rule_reads_the_same` cobrem os dois lados. O rótulo é
`[^─│]+`: um `│` no meio é borda de painel, não nome.

## O `wire.jsonl` do Kimi não é um transcript bem-comportado

— duas armadilhas medidas em
  14/08/2026, as duas em produção, na mesma sessão:
  - **Nem toda escrita é turno.** O hook grava `idle` no `Stop` e `state.corrige_ocioso_kimi`
    promovia pra `working` sempre que o arquivo fosse mais novo que o marcador (é o que cobre o
    prompt ENFILEIRADO na TUI, que não dispara hook nenhum). Só que o Kimi grava `config.update` —
    o system prompt inteiro, ~90KB — com a sessão parada: turno fechou 08:28, o `config.update` caiu
    08:40 e a sessão ficou "em execução" com o pane no prompt. Agora o mtime é só o **portão barato**
    (um `stat` por poll) e quem decide é `_kimi_turno_aberto`, que lê o **fim** do arquivo até a
    primeira fronteira de turno: `turn.ended`/`turn.cancel` = parada, `turn.prompt`/`turn.steer` =
    andando (levantado sobre todos os wires da máquina: não há outro `turn.*`). O regex é só filtro
    barato — quem decide é o `type` de TOPO da linha, via json, senão uma msg CITANDO
    `"type":"turn.ended"` vira fronteira.
  - **O main fica MUDO quando delega.** Subagente (tool `Agent`/`AgentSwarm`) roda no mesmo
    processo mas escreve no wire DELE (`<sessão>/agents/agent-N/wire.jsonl`); o
    `agents/main/wire.jsonl` não recebe uma linha enquanto isso. E quando um subagente termina, o
    hook `Stop` dispara com o `session_id` da SESSÃO — marcando `idle` no meio do turno do main.
    Foi essa dupla que fez a mesma sessão aparecer "pronta" com o terminal mostrando
    `Running 2 agents`, três vezes. Por isso o mtime não decide nada: quem decide é a fronteira de
    turno do main, e prova de vida (no caminho degradado) é o mtime mais novo entre TODOS os
    `agents/*/wire.jsonl`. Quem for mexer em estado do Kimi: **o wire do main não é a sessão**.
  - **`tool.result` não tem `uuid`** (só `parentUuid` e `toolCallId`), e o parser mandava `id=""`.
    O front deduplica evento **por id** (`Chat.svelte`, `idIndex`), então os 205 resultados de uma
    sessão real disputavam o MESMO slot: cada um apagava o anterior. Dois estragos ao mesmo tempo —
    todo card de ferramenta preso em "Executando…", e o card do **AskUserQuestion reabrindo depois
    de respondido** (o front deriva "respondida" da presença do `tool_result`; quando a ferramenta
    seguinte tomava o slot, a pergunta voltava a parecer pendente). Id agora é `res:<toolCallId>`.
    O teste antigo não pegou porque fabricava um `uuid` que o Kimi nunca manda: **ao escrever teste
    de parser, copie o shape do wire real**, não o que a doc sugere.

## Furar a fila do Kimi (steer)

(`terminal_input.steer_now` + `POST /api/sessions/{name}/steer` +
  o chip `⏳ N na fila · mandar agora` no `Composer`): msg enviada com a sessão trabalhando fica na
  fila da TUI do Kimi ("↑ to edit · ctrl-s to steer immediately"); o `ctrl-s` a injeta no turno em
  curso — vira `turn.steer` no wire, no MESMO turnId, com o `context.append_message` de user de
  sempre (por isso o dedup da fila durável não muda nada). Medido: o ctrl-s promove a fila
  **inteira** de uma vez (duas msgs entraram como um bloco só), e com a sessão parada é no-op. É
  tecla avulsa, não parâmetro do envio: a decisão "essa não espera" vem DEPOIS de já ter mandado. O
  número do chip conta as bolhas translúcidas — eco local (`pending`) **mais** os eventos
  `queued-` da fila durável; só o eco local dava 0 (ele some em ~1s, quando o `queued-` chega) e o
  chip nunca nascia. 409 fora do Kimi.

## Prévia ao vivo: sidecar do agente primeiro, pane depois

(`preview.read_sidecar` +
  `scripts/pi/hangar-state.ts`): mesmo contrato da statusline, agora pro texto **em voo**. A extensão do
  Pi recebe o bloco do assistente token a token (`message_update`) e publica o **último bloco de
  texto** em `<config>/.hangar-preview/<stem>.json` = `{"text", "ts"}`; `PreviewBroker._loop`
  o prefere e só cai no `capture-pane` quando não há sidecar. É o que tira a prévia do Pi da
  adivinhação: todo o `extract_assistant_text` (verbo de ferramenta, caixa do composer, spinner,
  painel de Todos) existe só pra separar prosa de desenho de TUI, e um quadro do spinner em `*`
  ASCII — fora de `SPINNER_GLYPHS` — já fez a prévia engolir a linha de status **e o painel de
  tarefas inteiro** (03/08/2026). Quatro coisas que o desenho decide de propósito: (1) `""` é
  **resposta** ("não há nada em voo"), `None` é ausência (cai no pane) — tratar os dois igual traria
  de volta o bloco já commitado como bolha duplicada; (2) publica o **último** bloco, não a soma —
  mandando a soma, `sse.preview_is_committed` vê o commitado como prefixo da prévia e engole tudo;
  (3) a extensão coalesce em 150ms e `unref()` o timer, porque `message_update` dispara por token e
  um timer pendente não pode segurar o processo do Pi vivo; (4) teto de idade de 10min, pro caso da
  extensão morrer no meio do turno — aí o pane volta a mandar em vez de congelar a última frase.
  Vale o mesmo aviso da statusline: **sessão Pi já aberta só publica depois de `/reload`**. O
  **Claude Code também publica** desde 17/08/2026: `hooks/preview_hook.py` (instalado pelo
  `hook_installer.ensure_preview_hook_installed` no startup) escuta o evento `MessageDisplay`
  (Claude Code ≥ 2.1.152 — deltas INCREMENTAIS do texto em exibição, medido: 5 parágrafos = 6
  eventos com `index` crescente e `final` no último, markdown cru) e grava o mesmo sidecar; o
  `Stop` zera. O acúmulo entre eventos vive no próprio sidecar (`message_id` gravado junto), e
  texto com `agent_id` (subagente) nunca é publicado. Sessão Claude já aberta não relê hooks →
  segue no pane até reiniciar; a raspagem inteira do `extract_assistant_text` vira plano B, não
  código morto. Codex nunca raspou pane (app-server).

  Em 19/09/2026, no Windows, oito deltas concorrentes com leitura retardada preservaram apenas
  um; segurar o destino aberto por 100 ms também fez perder uma atualização. O hook agora
  serializa o ciclo pelo `msvcrt` no Windows, usa `atomico.substituir` para a janela de leitura
  concorrente e registra a classe da falha sem conteúdo da conversa. A espera da trava usa
  tentativas curtas: o `LK_LOCK` impunha um segundo por tentativa. O `Stop` usa a mesma trava.

  O `/btw` também confere que o diálogo fechou após Esc. Tecla recusada, captura ilegível ou
  diálogo ainda aberto viram erro; não se repete Esc às cegas, pois poderia abortar o turno.

## O diálogo de confiança do Claude Code, e as três coisas que ele derrubava

(medido 06/09/2026,
  claude 2.1.263, com o pane real capturado em `tests/fixtures/pane_trust_dialog.txt`). Sintoma no
  Windows: sessão criada pelo app numa pasta nova morria sozinha e o app dizia "sessão não
  encontrada"; o chat de outra ficava em "reconectando" para sempre. São três defeitos em fila, e o
  segundo e o terceiro valem em qualquer sistema:
  - **A chave do pre-trust é o caminho com barra NORMAL no Windows.** No bundle do CLI,
    `function uN(e){let t=B(e); if(L()==="windows") return t.replaceAll("\\","/"); return t}` é quem
    monta a chave de `projects` no `.claude.json`. O `_pretrust_cwd` gravava o `cwd` cru — que vem do
    `fs.py` como `str(Path(...))`, com contrabarra —, então escrevia uma chave que ninguém lê e o
    diálogo aparecia mesmo com o pre-trust rodando. Hoje passa por `registry._chave_trust`. (A outra
    metade dessa armadilha, "escreveu no ARQUIVO errado", já estava fechada em `tmux.claude_json_de`.)
  - **`is_overlay` não via o diálogo porque olhava as 8 últimas linhas de um pane cheio de branco.**
    A caixa ocupa 16 linhas de um pane de 30 e o resto fica vazio; `capture-pane` devolve a altura
    inteira, então a janela de 8 linhas pegava só branco e o gate respondia "tela livre". Com isso o
    `deliverable` liberava, o envio digitava às cegas e o Enter caía em **"No, exit"** — que é a
    opção sob o cursor, porque o CLI desenha esse diálogo com `cancelFirst:!0, focus:"cancel"`. E as
    opções vêm com `hideIndexes:!0`, sem `1.`/`2.`, então `classify` nunca as vê como menu: o
    `is_overlay` é a única defesa. Quem responde "quais são as últimas 8 linhas" agora é o
    `state._rodape`, que descarta as em branco do fim — a mesma correção que o `_pane_tail` do
    `terminal_input` já tinha, e que o `_menu_block` (o gate do picker do Pi) também precisava.
  - **Quarta porta do mesmo defeito: `_composer_regiao`** (medido 08/09/2026, numa máquina Windows,
    parear `pss` com `pmw`). Numa sessão **recém-aberta** o Claude Code desenha o composer no ALTO
    da tela e o resto do pane vem em branco; a distância da régua de baixo até o fim estourava
    `_COMPOSER_FUNDO = 8` e a região era dada como ilegível. Consequência: `_deliver` não conseguia
    provar a entrega do prompt do grupo em NENHUM membro, `pair_session` reverteu o grupo e devolveu
    502, e a tela dizia só "Falhou o pareamento com pss." O log traz a geometria exata —
    `reguas=5,7 fundo=16` e `reguas=11,13 fundo=10`, panes de 23 linhas —, reproduzida com um pane
    fabricado nesses números. Aqui, com a janela cheia, o fundo é 4–7: por isso nunca apareceu no
    Linux. Hoje `_linhas_uteis` apara as brancas do fim antes de medir, e o **diagnóstico usa a mesma
    poda** — medindo o pane cru ele reportaria um fundo que a decisão real não usa.
  - **O `catch` do `PairSheet` jogava fora o motivo.** Todos os erros do `/pair` vêm em envelope
    traduzível (`erro_sessao_nao_encontrada_detalhe`, `erro_pareamento_desfeito`,
    `erro_pareamento_tarefa_existente`) e `lerErro` já os resolve antes de virar `Error` — o
    `catch` sem variável descartava isso e deixava a tela sem nada para consertar. Mesmo defeito no
    `doLeave`, corrigido junto.
  - **`Baixar-Dist` só falava no sucesso.** Cada `return $false` (sem tar/curl, sem git, `frontend/`
    sujo, sha do CI de outro commit, tar quebrado) era mudo, e o `npm ci` de um minuto e meio
    começava sem explicação — quem trocou para baixar o dist do CI não tinha como saber se o
    download nem foi tentado. Agora cada desistência imprime o motivo, nos dois instaladores; no
    `install.ps1` isso só é seguro porque `Nota` é `Write-Host` e não entra no valor de retorno.
  - **`awatch` numa pasta que ainda não existe derruba o SSE em laço.** `projects/<slug>` só nasce
    quando o agente escreve; até lá o `follow()` levantava `FileNotFoundError`, o `pump` mandava o
    erro pro cliente, o EventSource reconectava e caía no mesmo erro. O `TranscriptTailer.follow`
    espera a pasta em vez de estourar (o `mkdir` que o adapter do Codex já fazia era o mesmo
    problema, resolvido só naquele caminho).

**A confiança é por conta, e a troca de conta não a levava** (medido em 04/10/2026, Windows com
psmux, Claude Code 2.1.289, `main` em `d3043870`). Uma sessão criada numa conta e levada a outra
por `POST /api/sessions/{name}/conta` reabria no diálogo, com "No, exit" sob o cursor, e a lista a
mostrava como `awaiting_input`. Só acontece com pasta que a conta de destino nunca abriu e que não
está debaixo de outra já confiável nela: a mesma troca com a pasta dentro do perfil do usuário
abriu direto, e por isso o defeito passava despercebido. Com a marcação, a mesma troca numa pasta
nova abriu sem o diálogo. Não medido: a sessão sem terminal sobe com `claude -p`, que pelo código
não passa pelo diálogo, e ficaria com a pasta por marcar até virar terminal.

## Preferência da barra do Claude Code (07/09/2026)

o card de Harnesses abre **Opções**
(`HarnessOpcoes.svelte`), com rascunho e Salvar no servidor selecionado. `claude_statusline_update`
vem ligado; desligado, o instalador preserva `statusLine`. Linux e Windows chamam a mesma rotina
stdlib Node (`scripts/configure-statusline.cjs`), que lê `runtime-config.json` sem backend,
respeita `CLAUDE_CONFIG_DIR`, faz backup e grava sem BOM. Preferência inválida não vira autorização
para sobrescrever a barra. Salvar só muda a preferência, não o comando atual.

## Marketplace nativo com outro nome (07/09/2026)

o Claude Mem declara `thedotmack` no manifesto
Claude e `claude-mem-local` no do Codex. Nome do plugin + origem confirmada identificam o alias;
`registro.plugins` continua indexado pela fonte Claude, e `id_codex` acompanha o destino real nas
operações e na checagem de skills habilitadas. Não associar só pelo nome e não esquecer o destino
ao desabilitar: isso deixaria o plugin antigo executando. Mais de um alias possível é erro.

## Hook do `security-guidance` no JSON estrito do Codex (07/09/2026, PR #3)

o plugin 2.0.7
provocava dois erros medidos no Codex 0.153.4: `SessionStart` emite anúncio `async` + resposta
com `metrics`, e `Stop` também emite `metrics`. São extensões do Claude, recusadas pelo JSON
estrito do Codex. Só esse plugin recebe `codex-hook-json.py` (instalado em
`<codex>/.hangar-hooks/`); bloqueios, contexto e exit code são preservados, sem autoaprovar os
comandos novos. O PR também COPIAVA `~/.claude/hooks` inteiro pela área de importação e publicava
cópias com manifesto em `~/.codex/hooks` — mesmo bug que o `13ed4251` do mesmo dia já fechava com
symlink (`codex_hooks_arquivos`). Ficou o symlink, decisão do usuário: uma fonte só, sem cópia
pra envelhecer entre reconciliações. A parte de cópia foi retirada na integração do PR.

## Hooks em subpastas (07/09/2026)

`codex_hooks_arquivos` preserva o caminho relativo inteiro,
também ao atualizar cópias no Windows. A verificação anterior exigia o pai imediato `hooks/` e
ignorava `gitnexus/gitnexus-hook.cjs`: os comandos importados existiam, mas o arquivo não.
Captura de `hook/completed` no app-server confirmou falha nos dois hooks do GitNexus; execução
direta mostrou `MODULE_NOT_FOUND`, código 1. Homônimos na raiz não substituem arquivos de
subpastas; caminhos resolvidos fora de `hooks/` continuam excluídos. A restauração do arquivo
mantém o comando aprovado no Codex.

## Clone reduzido de marketplace no Codex (07/09/2026)

o clone completo do Claude Mem levou
44,09 s e trouxe 461 MiB nesta máquina; o atualizador nativo encerra o clone após 30 s. Clone
raso manual levou 12,44 s, mas o CLI não oferece `--depth`. A opção nativa `sparse_paths =
[".agents", "plugin"]`, no marketplace `claude-mem-local`, usa `--filter=blob:none` e checkout
das pastas necessárias: cadastro em 2,42 s, atualização em 3,21 s, mesma origem GitHub.
Essas pastas são específicas desse catálogo; não são padrão para marketplaces alheios.
O importador nativo considera opções de clone diferentes como outra origem, mesmo com a URL
igual, e recusava reimportar o plugin já instalado. O reconciliador agora dispensa a importação
quando nome e origem comprovam a instalação nativa, inclusive com alias; atualização, reparo,
habilitação e desabilitação continuam pela mesma esteira. Plugin ausente continua sendo importado.

## Avisos da reconciliação Codex (10/09/2026)

uma entrada MCP que já contém todos os campos
convertidos da fonte pode ser adotada mesmo com complementos locais (`tools`, timeout, variáveis
extras). O manifesto guarda só os campos da fonte; divergência em qualquer um deles continua
preservada com aviso. Um link pessoal que resolve exatamente para a skill descoberta, como o alias
antigo `~/hangar`, fica intacto e sem aviso, mas não é adotado como gerenciado: equivalência não
transfere propriedade. Falhas do CLI com código não zero, JSON inválido ou `errors` no JSON deixam
um diagnóstico privado em `<codex>/.hangar-diagnosticos/cli-<hash do comando>.log` (0600 no POSIX,
última falha por comando, caudas limitadas a 8 KiB por saída). O log do serviço mostra somente o
código e o caminho; stdout/stderr brutos podem conter tokens e não vão para ele. Em 10/09, o erro
de atualização do Claude Mem não reproduziu na nova tentativa: catálogo local e HEAD remoto
coincidiram e a reconciliação terminou `ok`, sem erros nem marketplaces pendentes. Isso comprova a
recuperação, não a causa da falha anterior, cujo detalhe não foi preservado pelo backend antigo.

## O que segura a abertura de uma sessão Codex é a TUI parada num widget, não a sincronização


(medido 10/09/2026, codex-cli 0.153.4 → 0.154.0). Com o CLI desatualizado a TUI abre com
"✨ Update available … Press enter to continue" ANTES do `thread/start`; sem thread não há
sidecar, e o app fica em "A conversa do Codex ainda não começou" para sempre (150 s medidos, sem
sidecar). O cartão de seletor pré-thread já existia (`menu_codex` + lista), mas só reconhecia o
rodapé "press enter to confirm" dos seletores de hooks/permissões — o do aviso de update é
"continue", e no pane estreito (terminal do celular anexado) a opção 1 quebra em 3 linhas, que
fechavam o bloco com 1 opção. Hoje os dois rodapés valem e continuação indentada sem número cola
na opção anterior. Escolher "Update now" pelo app funciona, mas o Codex SAI depois de atualizar e
o pane morre: a sessão some da lista e precisa ser criada de novo. A reconciliação custa ~20 s
quando roda (fingerprint mudou, 6 h do marketplace, ou 5 min após falha); em 10/09 ela rodou em
toda abertura porque o "auto-upgrade was in flight" do marketplace `ecc` contava como falha
(`fcdd382b`). Sem o aviso de update, a thread abriu em 21 s nesta máquina. Pendente, visto uma vez no
0.154.0: o seletor de hooks passou a desenhar SEM número (`›    Review hooks`), e `menu_codex`
exige `N.` — captura real do widget antes de mexer.

## Identidade durante a abertura do Codex (12/09/2026)

O provider escolhido na criação entra no ambiente do terminal como `CP_PROVIDER`, lido junto
dos panes. Enquanto nenhum processo de agente é reconhecido, essa escolha prevalece; depois,
o processo reconhecido permite a troca normal de provider. O registro não depende do backend
continuar vivo. A criação também descarta o snapshot anterior e retorna `tracked=false` enquanto
não há transcript.

O journal mostrou `hangar-2` aberta como Claude às 09:20:08 e reconhecida como Codex apenas às
09:20:12: o fallback chegou a servir um JSONL antigo. Em `hangar-3`, o primeiro histórico e SSE
receberam 404; a primeira lista já trazia a thread pronta e não disparava recuperação. O Chat
agora recupera esse 404 uma vez por transcript disponível, sem depender de observar a fase sem id.

Na integração sem mudanças, o teste com cliente simulado passou de três inventários e uma
detecção para um inventário e nenhuma detecção. Importação e atualização ainda renovam o
inventário. Isso mede chamadas evitadas, não ganho de tempo da abertura real.

## Instruções nativas (07/09/2026, PR #3)

`codex_instrucoes.py` prepara `AGENTS.override.md`
— nome que o Codex 0.153.4 lê no lugar do `AGENTS.md` da mesma pasta — como link para o
`CLAUDE.md` global (`<codex>/AGENTS.override.md`) e dos projetos registrados no `config.toml`; o
lançador prepara também os escopos raiz→cwd antes de subir o app-server. `CLAUDE.MD` é a segunda
opção. Sem permissão de symlink, usa cópia gerenciada que é
atualizada na próxima preparação. Isso INVERTE a decisão de 06/09 (bloco "leia o CLAUDE.md" no
`AGENTS.md`, custo de as regras não estarem no primeiro token): o bloco antigo sai com backup e o
`CLAUDE.md` inteiro entra no primeiro request — por isso `project_doc_max_bytes` sobe pra pelo menos
1 MiB, crescendo com as fontes conhecidas e preservando limite maior já configurado. Dois custos
aceitos pelo usuário: ~110 KB de contexto por sessão Codex, e um arquivo untracked na raiz de cada
repo com `CLAUDE.md` — o mesmo problema dos anexos de 31/08, mitigado aqui gravando
`AGENTS.override.md` no `.git/info/exclude` do repo (`_excluir_do_git`), que é local e não
versiona. Onde já existe `AGENTS.md` de verdade, ele deixa de ser lido pelo Codex (o override
substitui, não soma). Teste com CLI real captura a primeira requisição em servidor local, sem
modelo: global + projeto acima de 180 KB presentes, AGENTS preteridos ausentes. Projeto novo
aberto pelo IDE/CLI cru precisa ser registrado e reconciliado antes de ganhar prioridade sobre um
AGENTS existente. Sessões já abertas conservam o contexto inicial. Falha na preparação (override
pessoal sem fonte Claude, `config.toml` ilegível) não impede a TUI de abrir: sai aviso no stderr do pane.

**Decisão de 29/09/2026.** Ativar sincronização no menu Harness autoriza atualizar as instruções
do Codex a partir do Claude mesmo quando o destino divergiu do registro. O destino anterior é
guardado em backup restrito por conteúdo; fonte externa de um symlink não é modificada. A
preparação automática dos aliases também respeita o interruptor, inclusive pelo lançador.
Reconciliar manualmente continua sendo uma ação explícita, permitida com o interruptor desligado.
A falha observada foi um `ValueError` na preparação global: o `AGENTS.override.md` da instalação
principal diferia do hash registrado, e a rodada parava antes de atualizar skills. A alteração
de data sozinha não explica a falha. Regressões acrescentadas para fonte autoritativa, backup,
interruptor desligado, link externo e segunda preparação; testes não executados nesta tarefa.
Na conferência real, a integração principal concluiu com estado `ok`, sem erros; o override
passou a coincidir com o conteúdo expandido do Claude e com o hash registrado, com backup do
destino anterior. A skill `orquestrar-auto` foi instalada na principal.
O preparo da conta `jefferson-felizardo` também copiou as instruções e a skill; permaneceu
parcial pelos conflitos de origem dos marketplaces, com confiança dos hooks pendente. Essa
pendência não foi autoaprovada nem tratada como sincronização completa da conta.

## Preparo do Codex sem repetir etapas inalteradas

**Regra vigente.** Conferir inventário antes de materializar os recursos da conta. Separar as
assinaturas de instruções, plugins, fragmentos e skills; executar apenas categorias alteradas.
Falhas de plugins continuam visíveis e são reutilizadas por até 300 s quando fonte, destino e
CLI permanecem iguais; forçar atualização ou mudar o inventário invalida esse prazo. Confiança
pendente dos hooks é consultada separadamente, sem autoaprovação. Importação de memória ativada
continua passando pela etapa de fragmentos. Fonte e destino são reconferidos antes de guardar o
inventário validado; erro de validação não vira sucesso de cache.

**Conferência em 29/09/2026.** Na conta `jefferson-felizardo`, uma chamada real de preparação
levou 60,482 s antes da alteração. Após aplicar, a primeira rodada levou 50,127 s para preencher
os registros; as duas seguintes levaram 2,027 s e 1,017 s. Medição pelo tempo monotônico entre o
POST `/api/codex-contas/jefferson-felizardo/prepare` e o estado final, consultado a cada segundo.
São uma amostra anterior e duas posteriores com cache preenchido, não percentis nem promessa
para importação completa. O estado permaneceu parcial, com conflitos de plugins e confiança
pendente visíveis. Regressões foram acrescentadas para inventário, cache, aprovação dos hooks e
mudança de skill isolada; testes automatizados não foram executados.

## Perguntas assíncronas do Codex (11/09/2026, CLI 0.154.0)

`request_user_input_async`
chega como `agentMessage` com `delivery: "async"` e `questions`, não como pedido JSON-RPC
`item/tool/requestUserInput`. `async_questions.py` acompanha cada pergunta por thread/item/índice;
o `thread/resume` repõe o histórico antes de reaplicar eventos recebidos durante a leitura.
O backend acompanha também sessões recém-abertas pelo terminal. `pending_questions` alimenta os
avisos das listas e abas sem trocar `working` por espera; o formulário existente recebe a pergunta.
A resposta usa o mesmo `turn/start` da TUI, dirigido à thread original, com `> título\n\nresposta`.
Cada pergunta é independente, inclusive quando uma chamada contém várias. O eco da resposta local
não responde de novo outra pergunta com título igual. Provas com app-server/TUI reais e provedor
local falso confirmaram envio durante o turno, resposta após seu fim e recuperação após reconexão.
Limites nativos medidos: “Pular” só altera a memória da TUI, sem evento/histórico; uma resposta por
outro cliente não fecha o widget já aberto no terminal. O Hangar reconhece respostas do terminal
pelo histórico, mas não inventa confirmação de descarte.

Em 01/10/2026, com o CLI 0.159.3, a resposta do terminal passou a chegar em
`<send_user_message_question_reply>`, com uma lista JSON cujo `questionItemId` identifica a
ferramenta, a chamada e o índice da pergunta. Reconhecer apenas o título deixava a pergunta
pendente depois da resposta. O backend resolve esse formato pelo ID e mantém a leitura do
formato anterior para o histórico. A recuperação do histórico real confirmou zero perguntas
pendentes; os testes cobrem títulos repetidos, respostas em lote e resposta durante a leitura.

## Aviso de espera do Codex (11/09/2026, CLI 0.154.0)

o popup “Giving this request a little
extra thought” vem de `model/safetyBuffering/updated`, não de uma pergunta ou temporizador local.
`showBufferingUi` alimenta `codex_buffering` no estado compartilhado, exibido como aviso informativo
no web e no app nativo. O turno continua trabalhando; delta não vazio, mensagem completada, fim do
turno ou sinal `false` retiram o aviso. Eventos de outra thread/turno não o alteram. Reabrir o chat
preserva o estado no backend. Limite medido com CLI real e provedor local: um cliente novo não
recupera buffering por `thread/read`/`thread/resume`; reiniciar o backend perde o aviso já emitido.

## - `adapters/codex/` — one loopback WebSocket app-server per Codex sess

- `adapters/codex/` — one loopback WebSocket app-server per Codex session; the backend consumes
  structured JSON-RPC events while a `codex --remote` TUI for the same thread runs inside tmux.
  **Controles nativos do chat (07/09/2026, CLI 0.153.4):** `thread/read` e `thread/resume`
  informam `reasoningEffort`, enquanto `thread/settings/updated.threadSettings` usa `effort`.
  `thread/settings/update` compartilha modelo, esforço e `collaborationMode` com a TUI; o
  `turn/start` herda esses valores, pois reenviar o sidecar sobrescreveria uma escolha do terminal.
  `skills/list` fornece nomes e caminhos de entradas `UserInput` do tipo `skill`; o texto `/nome`
  permanece no histórico para reconciliar os ecos da fila. `turn/steer` exige `expectedTurnId`:
  uma orientação para um turno encerrado falha, preservando a mensagem. Contexto estendido usa
  `model_context_window=1000000`, com restauração do valor anterior e sem editar o catálogo;
  o Codex aplica `max_context_window` de cada modelo (Astra/Sol: 872000 nessa instalação).
  **Reconexão do modo (09/09/2026, revisão do PR #4):** `thread/read` não devolve
  `collaborationMode`, e uma conexão nova não recebe o retrato de `thread/settings/updated`.
  Prova com dois clientes do CLI real, sem inferência: o primeiro escolheu Planejar e o segundo
  não recebeu essa escolha. O último `turn_context` do rollout é histórico, não estado atual.
  Até uma notificação ou troca confirmada, o backend devolve `null` e o seletor mostra modo
  desconhecido. A descoberta do plano Claude acontece nas transições de estado, não a cada
  mensagem/ferramenta: cada chamada percorre o transcript inteiro.
  **O app-server é do PANE, não do backend** (`scripts/hangar-codex-tui`, o lançador único que o
  backend e o terminal chamam igual): ele escolhe a porta, sobe o servidor em segundo plano, roda a
  TUI em primeiro plano — nunca `exec`, que é o que o deixaria sem quem matar o servidor na saída —
  e grava `endpoint`+`app_pid` no sidecar junto de thread/rollout/cwd. O backend só se **liga**
  nele (`AppServerClient.connect`), conferindo o pid antes: porta de loopback é reciclada, e
  conectar só pelo endereço pode cair num processo alheio. Pid morto é sessão morta, não sessão a
  reconectar. Por isso criar sessão Codex passou a ser o caminho normal de criação (`registry.create`
  com `provider="codex"`, transcript vazio como Pi/Kimi) — não há mais `create_codex`. Duas armadilhas
  que sobram: o `codex` está no `_EXEC_PROVIDER` porque entre o pane nascer e o sidecar existir o pane
  cairia no default `claude` e seria casado com o transcript do Claude do mesmo diretório; e nessa
  janela `info.jsonl` é `None`, então tudo que deriva chave do transcript (`session_key`) tem que
  desviar — `session_key(None)` levanta `TypeError` e derrubaria a lista inteira, de todas as sessões.
  E uma terceira, medida em 04/09/2026: **a TUI só sobe depois de a porta do app-server aceitar
  conexão** (`_esperar_porta`). O `codex --remote` conecta UMA vez e, recusado, sai com 1 — o pane
  morre, o tmux imprime `[exited]` e o `hangar-codex` apaga a sessão em menos de 1s. Só aparece com a
  máquina carregada (load ~5 com suítes rodando): aí o servidor perde a corrida pro bind e a TUI
  chega antes. Com a máquina folgada nunca reproduzia, em nenhum terminal.
  **A fila de notifications do app-server tem UM consumidor por sessão** (`_bombear`, 04/09/2026):
  cada SSE é um ouvinte que recebe cópia dos `StateEvent`s, e o primeiro evento é o retrato do
  que a sessão já sabe (estado + status line). Antes, cada SSE lia a fila direto e o comentário
  dizia que "ainda convergem" — o contrário: `queue.get()` entrega cada delta a UM consumidor, e
  desktop + celular no mesmo chat mostravam metade da frase cada ("Faria em pequenas, o atual."
  no lugar de "Faria em mudanças pequenas, preservando o comportamento atual.", reproduzido com
  o `AppServerClient` real). E sem o retrato inicial, reabrir o chat no meio do turno deixava a
  tela sem estado nem contexto até a próxima notification — o que parecia "o Codex perdeu o
  contexto" com o rollout íntegro. A bomba morre com o último ouvinte (drain-on-complete
  continua acoplado a haver um SSE aberto, como antes).
  **O `rtk` no `hooks.json` do Codex passa por `scripts/codex-hook-allow.py`**: o rtk 0.43.0
  devolve `updatedInput` sem `permissionDecision` quando reescreve só um pedaço de um comando
  com `;`; Claude Code aceita, o Codex recusa a reescrita ("PreToolUse hook returned
  updatedInput without permissionDecision:allow") e roda o original. Só o rtk é embrulhado —
  um pipe em volta de outro hook esconderia o rc=2 com que ele bloqueia.
  **O provedor `command-code` que o `agentes_sync` grava no `config.toml` do Codex não serve ao
  Codex** (medido 04/09/2026): o gateway só tem `/chat/completions` — `/responses` é 404 em
  `/provider`, `/provider/v1`, `/v1` e na raiz — e o codex-cli 0.153.1 recusa `wire_api = "chat"`
  ao carregar a config. O bloco fica lá como promessa vazia; testar Codex noutro modelo hoje só
  com provedor que fale Responses API (a OpenCode fala, mas a conta estava sem saldo).

## Memórias do Claude no Codex (11/09/2026, CLI 0.154.0)

**A memória do Claude só é detectada com uma CONVERSA ao lado dela, e não basta o arquivo existir.**
O `externalAgentConfig/detect` lê `~/.claude/projects/<pasta>/memory/`, mas ignora o projeto que não
tenha um `.jsonl` irmão. Medido no stage, mesmo projeto, um candidato por vez: só `memory/` →
`tipos=[HOOKS]`, memória 0; `memory/` + um `.jsonl` de 11 linhas contendo apenas `mode`, `attachment`
e `system` → memória 0; o mesmo com um de 27 linhas contendo `user` e `assistant` → reconhecido. Por
isso `copiar_memorias` leva o menor transcrito que tenha `"type":"user"`, e não o menor arquivo: o
menor era justamente o mais provável de ser uma sessão que abriu e nunca conversou. O caso real foi
o próprio `hangar`, 57 memórias copiadas e descartadas em silêncio, com a reconciliação terminando
`ok`. Esse silêncio é o risco permanente daqui, porque o critério do detector não é documentado:
por isso a importação compara o que copiou com o que `details.memory` devolveu (confirmado contra o
CLI: lista de strings com a chave de pasta sanitizada) e avisa a diferença, seja qual for a causa.
Transcrito todo do projeto seria 1.179 MB; um por projeto, escolhido por tamanho, 35 MB; escolhido
por ter conversa, 11,6 MB.

**A consolidação não roda abaixo de 25% de cota, e decide antes de gastar.**
`codex_memories_write::guard`: `skipping memories startup because Codex rate limits are below the
configured threshold min_remaining_percent=25`. A conta padrão desta máquina passou o dia em 12% e
nunca consolidou, o que parece defeito e é proteção. `account/rateLimits/read` lê a cota sem custo e
diz de antemão se vai rodar. O worker só sobe com a TUI; `codex exec` não o dispara.

**A memória só vale a partir da SEGUNDA sessão.** O `memory_summary.md` é injetado na abertura, então
a sessão que manda consolidar não enxerga o próprio resultado — duas verificações deram falso
negativo por isso antes de a sessão seguinte responder certo. A tela diz isso ao lado do interruptor.

**O índice injetado é truncado em 5.000 tokens.** É o teto que faz importar todos os projetos não
inchar o contexto: o corpo (`MEMORY.md`, `rollout_summaries/`, os resources importados) fica fora e
é lido por busca, com orçamento de 4-6 passos declarado no próprio prompt. Medido: sem memória, o
bloco não é injetado; com a memória desta máquina, 8.657 tokens contra 4.835 no mesmo prompt.

## Claude sem terminal vive num cano, não no backend (13/09/2026, CLI 2.1.270)

`adapters/claude_headless/cano.py` + `adapter.py`. O `claude -p --input-format stream-json` da
sessão sem terminal nasceu como filho direto do backend, com stdin/stdout em pipe. Consequência
medida: todo restart do backend (atualizar pelo app, `systemctl restart`, o restart de
desenvolvimento que acontece dezenas de vezes por dia) matava o processo; o `--resume` do prompt
seguinte recuperava a conversa, mas o turno em voo era interrompido e uma permissão pendente sumia
sem resposta. A sessão no tmux não tem esse custo — o pane é dono do processo, não o backend.

Dois desenhos foram considerados e um recusado:

- **Mover o adapter inteiro pra um "escravo" único** (todos os claudes num processo, backend
  cliente). Recusado por três motivos: o adapter é o código que mais muda (cada mudança
  reiniciaria o escravo e mataria as sessões do mesmo jeito); um processo pra N sessões é um
  domínio de falha pior que o tmux (uma exceção no laço de eventos derruba todas); e o protocolo
  ficaria largo (13 métodos, dois streams, prévia em pubsub de memória, cota, fila).
- **Um cano por sessão** (adotado). O que sai do backend é a parte que não muda: um script stdlib
  (~250 linhas) que sobe o `claude`, segura stdin/stdout e escuta num socket local (unix; TCP em
  loopback com token no Windows ou quando o caminho passa dos 107 bytes do kernel). O adapter,
  com toda a máquina de estado, fica no backend e reconecta. Sem replay de eventos: o cano vê os
  dois sentidos e sintetiza um **snapshot** do que está em aberto — última `system/init`, turno
  aberto (viu `user` sem `result` depois), `control_request` ainda sem `control_response` (a
  permissão pendente, literal), último `result` e `rate_limit_event`, cauda do stderr e o `rc`
  se o claude já saiu. Sem cliente, as linhas são descartadas; o snapshot carrega o que importa.

O que a sonda real confirmou, nesta ordem: sessão em modo `manual` pede permissão de `touch`;
`kill -9` no backend; backend novo religa (`aberto=True pendentes=1`) e a lista mostra
`awaiting_input` com a mesma pergunta; `Permitir` responde pelo cano; o turno fecha, o arquivo
existe, o status line traz custo e contexto (do `ultimo_result` do snapshot).

Armadilhas que custaram tempo:

- **`asyncio.open_unix_connection` tem teto de 64 KB por linha**, e o `control_response` do
  `initialize` passa de 100 KB. A leitura estourava com `ValueError`, o leitor morria com a
  exceção e o `wait()` do `finally` esperava pra sempre: sessão presa em `working`. Mesmo
  `limit=16 MB` que o subprocess já usava, e a leitura embrulhada em `try` — EOF ou erro viram
  "o cano sumiu", nunca leitor pendurado.
- **O cano nasce no escopo transiente do systemd** (`tmux._scope_prefix`), pelo mesmo motivo do
  tmux e do atualizador: sem isso o `systemctl restart` do serviço mata o cgroup inteiro, cano e
  claude juntos. Conferido em `/proc/<pid>/cgroup`: `run-p<pid>.scope`, fora do serviço.
- **Claude e cano no mesmo grupo de processos** (o cano é `start_new_session`, o claude não): matar
  o grupo pelo pid do cano mata os dois, e `close_sync` recebe o `meta` do sidecar (que o registry
  apaga antes) porque é nele que mora o pid — a sessão pode ser encerrada sem este backend jamais
  ter conectado nela.
- **Órfão mudou de sentido.** Antes, todo claude de backend anterior era órfão (marcador com o pid
  do pai). Agora órfão é só o cano cuja `HANGAR_CANO_KEY` não tem sidecar — os outros são de
  propósito, e o backend religa em todos na subida (`reconectar_todas`), senão a lista mostraria
  "ociosa" uma sessão parada numa permissão. Com o `hangar-server` de pé (dono único, 04/10/2026)
  quem abre os canos vivos é o Rust, ao entrar no modo `rust`; o `reconectar_todas` fica para o
  Python dono da porta (sem binário ou depois da desistência).
- **`makefile()` segura o socket**: fechar só o socket não entrega EOF ao outro lado. Cano e
  cliente de teste fecham os dois.
- **Cano de outra versão** (`versao` no snapshot): com a sessão ociosa o adapter reabre na hora;
  com turno ou permissão em aberto continua falando com o velho. O que nada resolve é atualização
  da CLI do Claude ou do próprio cano com sessão trabalhando — o processo tem que morrer; o portão
  é reabrir só ocioso.
- **Cliente novo substitui o ligado, em thread própria** (medido no Windows, 13/09/2026). O
  `servir` atendia em série: o segundo cliente conectava no TCP mas só recebia snapshot quando o
  primeiro saía, e `_conectar` desiste em 5s e `_spawn` dá `taskkill /T /F` no cano "mudo". Quem
  conectou foi a suíte do backend: os testes com `with TestClient(app)` rodam o lifespan, e o
  `reconectar_todas` leu os sidecars REAIS de `~/.hangar/claude-headless`. No log do backend,
  `hangar-2` 17:36:32, `hangar-3` 17:36:37, `hangar-5` 17:36:42 — 5s entre cada, na ordem dos
  arquivos, todos com `WinError 64`; o claude dessas sessões (pid 1012 na `hangar-5`) sumiu, e
  a `hangar-5` era a sessão que rodava a suíte, que morreu junto. O cano dela tinha sobrevivido ao
  restart do backend minutos antes (`cliente saiu 17:28:51`, `cliente conectado 17:28:58`), e o
  kill da sessão de teste pegou só a árvore dela. Os testes de cano pulavam no Windows (só socket
  unix); o de troca de cliente roda em TCP. Duas correções: `conftest` aponta `sessions._dir` pra
  pasta temporária na sessão inteira, e o cano atende cada cliente numa thread. Quem troca só
  derruba o socket antigo (`shutdown` acorda o leitor no Linux, fechar o descritor acorda no
  Windows); o `makefile` é fechado pela thread que lê dele — fechá-lo de fora, no Windows, espera
  o `readline` em curso segurando a trava que o leitor precisa.

## Claude sem terminal estaciona a sessão parada (13/09/2026, CLI 2.1.270)

O processo subia no primeiro prompt e ficava vivo enquanto o backend vivesse. A vigia do adapter
(`_vigiar_ociosas`, a cada 60s) encerra o de sessão parada há `_OCIOSA_S` sem turno, permissão,
pergunta, subida, drain, `/effort` pendente, troca de cano ou fila por entregar. O prazo de 65 min
fica acima da janela de 1h do cache do prompt: parada mais que isso, o próximo turno relê o
contexto de qualquer jeito, então religar não custa cota a mais. Não há campo parecido na tela de
Servidor, por isso é constante.

A prova real (haiku, prazo encurtado pra 20s, sidecar em pasta temporária) achou o furo que o
teste de unidade não pegava: a saída provocada por nós chama `saiu(None)`, o `returncode` fica
`None` e a sessão continua "viva" em `_sessions`. Sem SSE aberto pra tirá-la de lá, o prompt
seguinte encontrava a sessão morta, `deliverable` dava falso e nada subia. `_encerrar` (o que o
`_reabrir` já fazia) mata e tira da memória. Com isso: estacionou aos 21s, sidecar sem `cano`, o
prompt seguinte subiu com `resume=True` e respondeu a palavra combinada no primeiro turno.

## Claude sem terminal: Recarregar recicla o processo pra reler a config (16/09/2026)

Registrado o MCP `hangar` na conta, a sessão `hangar-2` (processo de 05:57) continuou sem as
tools: `/mcp` respondia "No MCP servers are configured", porque em `-p` a CLI lê `.claude.json`
só ao nascer e não tem reconexão. O único caminho que já existia era o `estacionar` dos 65 min.
`recarregar` faz o mesmo par (`_encerrar` + `acordar`, subida com `--resume`) na hora, atrás da
mesma guarda de ociosa da troca de modo. O motivo é calculado, não adivinhado: na subida o cano
guarda `config_marca` (sha1 do `mcpServers` do `.claude.json` + `settings.json` inteiro), e
`motivo_recarga` recalcula e compara (cache de 10s por sessão, porque o `state` sai a cada
evento). A primeira versão usava o mtime do `.claude.json` e disparou em toda sessão: o próprio
Claude Code reescreve esse arquivo a cada abertura (estatísticas, projetos), então "mudou depois
que o processo subiu" era sempre verdade. Prova numa sessão
descartável: motivo `None` ao nascer; `touch` no `.claude.json` → `config`; `POST /recarregar`
trocou o pid (633974 → 637293) e o motivo voltou a `None`; o prompt seguinte chamou
`mcp__hangar__quem_sou` e respondeu o próprio nome. Na tela o botão só aparece com motivo (pill
acima do composer); no menu "⋯" e na paleta ele fica sempre, bloqueado fora de ociosa.

## Claude sem terminal que não sobe: teto de subidas (13/09/2026, Windows)

Sessão com `engine` inexistente e prompt na fila gerou 179 quedas `rc=1` em cerca de 1 minuto. O
`_esperar_initialize` falhava, o `finally` drenava a fila, o `drain` chamava `send_prompt`, e ele
subia outro cano na hora. Agora `_spawn` conta subidas seguidas sem `initialize` bom: espera 5s e
depois 10s entre elas, e na terceira falha levanta `_SubidaEsgotada`. O `drain` marca a entrada
como `desistiu` ("não chegou — reenvie"), e o problema da última queda continua na faixa. A
contagem zera no `initialize` bom e no `acordar`, que só é chamado por ação do usuário.

A espera fica sob a trava de spawn, não no fim da subida. A primeira versão esperava só no
`_esperar_initialize`, e a prova real mostrou a segunda subida 0,7s depois da primeira: o drain do
SSE do chat aberto subia sem passar por lá. Outro furo da prova: `deliverable` dava verdadeiro pra
sessão com processo morto, então o `POST /input` chamava `send_prompt` direto, subia com a contagem
da subida anterior e segurava o request pela espera. Sessão morta agora não é entregável: o prompt
vai pra fila e o `acordar` sobe. Resultado medido: 3 subidas (0s, +6s, +17s), nenhuma nos 70s seguintes.

A faixa do chat mostra só a primeira linha do detalhe, e o detalhe começava com `rc=1`: a mensagem
`hangar-engine: motor 'motor-inexistente' não existe` nunca aparecia. O stderr vem antes, e o `rc`
no fim. O acento chegou íntegro (cano com fallback pra codepage do console).

## Push de "aguardando" do Claude sem terminal (13/09/2026, Windows)

O marcador `awaiting_input` era gravado e o loop pausava na hora, mas o push nunca saía.
`_do_notify_awaiting` lia o estado de `registry.list()`, que não calcula estado (sai sempre
`idle`); o teste de unidade forjava a lista já com `state`. Agora pergunta ao `snapshot` do
adapter. Prova real com VAPID temporário e um receptor local inscrito: o push chegou 4s depois da
permissão (1,5s de nova checagem + 2s de agrupamento).

Achado sem conserto: o `vapid_subject` padrão (`mailto:hangar@local`) é recusado pelo `py_vapid`
("Missing 'sub' from claims"). Servidor com VAPID e sem `CP_VAPID_SUBJECT` não manda push nenhum.

## Pensamento em voo no Claude sem terminal (13/09/2026, CLI 2.1.270)

Medido com `claude -p --output-format stream-json --verbose --include-partial-messages` (sonnet,
esforço máximo): com `--settings '{"showThinkingSummaries": true}'` e sem ela, o bloco `thinking`
chega vazio — 15 `thinking_delta` sem texto e só a assinatura. No código da CLI, a exibição
explícita (`--thinking-display`) vence; sem ela, sessão não interativa não lê a chave. Com
`--thinking-display summarized` chegaram 127 pedaços, 1160 caracteres, e o bloco caiu com texto no
`.jsonl`. Então até esta data a sessão sem terminal nunca mostrava pensamento, nem com a chave
ligada. O adapter passa a flag quando `pensamento.ler()` é verdadeiro, na subida do processo.

O texto em voo não usa a prévia: a prévia vira bolha de resposta e é deduplicada contra o texto
do transcript. `fonte_pensamento` e `fonte_ferramenta` (a chamada cujo pedido o modelo ainda
escreve, com o input parcial) são fontes `PushPreviewSource` à parte, com eventos SSE `pensamento`
e `ferramenta`. O servidor limpa quando o bloco cai no `.jsonl`; o front espera o evento real do
transcript pra tirar de cena, com 3s de carência, senão abria um buraco entre os dois.

## Claude sem terminal: segundo `initialize` no mesmo processo (04/10/2026, CLI 2.1.289)

O Rust que abre um cano já inicializado reenvia o `initialize`, porque o snapshot do cano não
marca `initialized` (só reaplica o `system/init`). Medido com o CLI real (`claude -p` em
stream-json com `--permission-prompt-tool stdio`, Haiku, conta `.claude-02-200`, sem cano nem
backend): o segundo `initialize` responde `success` com o mesmo corpo do primeiro (244
comandos), tanto logo depois do primeiro quanto depois de um turno completo, e o turno seguinte
sai normal. Diferente do Codex, que recusa com `-32600 "Already initialized"`. Então a reabertura
no Rust não precisa de tratamento especial: a resposta marca a sessão entregável como no
primeiro. Se uma versão futura recusar, o sintoma é `headless_nao_subiu` com a frase do CLI na
faixa (`claude.rs`, ramo `initialize` rejeitado). Teste de regressão:
`reopen_of_initialized_cano_becomes_deliverable`.

## Codex sem terminal: o app-server é do cano (14/09/2026, codex-cli 0.154.0)

`adapters/codex/sem_terminal.py` + o ramo `headless` de `adapter.py`. A regra "o app-server é do
PANE" existia porque a TUI era quem o matava ao sair; sem TUI o dono passa a ser o mesmo cano do
Claude sem terminal (`claude_headless/cano.py`), com `codex app-server --stdio` como filho. O cano
ganhou rastreio genérico de pedido JSON-RPC (`method`+`id` do filho, limpo pela resposta do
cliente ou por `serverRequest/resolved`), e é isso que faz uma aprovação pendente sobreviver ao
restart do backend. Medido:

- `initialize` repetido no mesmo processo (backend religando) devolve `-32600 "Already
  initialized"` — tratado como sucesso.
- `thread/start` pelo cliente funciona; `on-request` + `read-only` gera
  `item/commandExecution/requestApproval` (com `reason`, `command`, `cwd`) e `{"decision":
  "accept"}` libera. Thread aberta por RPC que nunca teve turno não tem rollout e o
  `thread/resume` da reabertura falha com `no rollout found` — o adapter abre outra e troca o
  sidecar.
- `approval_policy = "untrusted"` foi removido na 0.154: o app-server sai na hora com `Error:
  approval_policy = "untrusted" is no longer supported`. Só restam `on-request` e `never`, por
  isso os três modos do app se distinguem pelo sandbox — e sandbox não troca ao vivo por RPC
  (`codex_permissions.py`): fica no `-c` da subida e trocar de modo reabre o app-server ocioso
  com `thread/resume`. `approvalPolicy` vai em cada `turn/start`.
- Sonda real: turno com aprovação pendente → `kill -9` no backend → religou no mesmo cano
  (`pendentes=1` no snapshot) → `Permitir` pelo app → o comando rodou e o turno fechou. Sem
  resposta, o pedido ficou em aberto por 16s sem ninguém decidir por ele.
- Um cliente por cano: um segundo backend (ou sonda) apontando pro mesmo sidecar rouba a conexão
  do primeiro, que vê `conexao encerrada`. Sonda com o serviço no ar precisa de pasta de sidecars
  própria.

Fora do escopo por enquanto: renomear uma sessão Codex sem terminal (a rota passa pelo tmux),
sincronização de conta secundária na subida (segue os gatilhos existentes) e o botão de trocar
terminal ⇄ sem terminal, que é só do Claude.

## Identidade e permissão no Codex sem terminal (14/09/2026)

Revisão posterior: a troca de sandbox religa no cano sem reiniciar e confirma o estado antes de
encerrá-lo. Estado desconhecido, inclusive depois de uma reconexão pelo monitor, mantém o processo
e a permissão. A consulta inicial do Composer não pode sobrescrever uma leitura ou troca mais
recente confirmada pelo popover.

O Codex associa a confiança dos hooks à posição no arquivo. A reconciliação preserva posição e
agrupamento dos hooks inalterados: retirar os importados e reapendê-los após o `guard_tmux`
deslocava seus índices, e `hooks/list` passava a informar `modified`. Isso também impede a execução
no headless. A confiança já invalidada requer aprovação nativa explícita; o Hangar não regrava
`trusted_hash` para contornar essa aprovação. `exec_command` chega aos hooks como `Bash`, portanto
o nome da ferramenta não era a causa desse problema.

Scripts executados dentro de uma sessão sem terminal recebem `CP_SESSION_KEY`; o nome atual vem do
sidecar que contém essa chave. O Claude já fazia isso em `~/.hangar/claude-headless/`. O Codex
exportava apenas `HANGAR_CANO_KEY`, enquanto `hangar-preview` e `hangar-send` só procuravam o
diretório do Claude; ambos caíam no fallback de tmux, que não existe nesse modo. O Codex passou a
exportar a chave comum e os dois CLIs procuram também `~/.hangar/codex-sessions/`; eles ainda
aceitam `HANGAR_CANO_KEY` para as sessões abertas antes da atualização. Os testes usam sidecars
temporários e confirmam que rename/ausência de tmux não muda o nome resolvido.

O modo de permissão do Codex sem terminal é o `permission_mode` do sidecar; ausente significa
`Full Access`, igual à sessão Codex com terminal. A leitura é segura durante um turno porque não dirige `/permissions` nem reinicia
o app-server. A troca continua recusada durante o turno, pois mudar o sandbox exige reabrir o cano.
O formulário agora oferece os três modos e envia a escolha na criação; o Composer carrega o valor
na montagem e o mostra também no layout compacto do PWA.

## Voz Codex no web

(`codex_voice.py`, `CodexVoice.svelte`, `lib/codexVoice.ts`, 10/09/2026):
  é beta e opt-in por servidor: `codex_voice_beta` nasce `false` no runtime config e aparece em
  Harnesses → Codex → Opções. Desligada, o botão não monta e o backend recusa WebSocket e catálogo
  de vozes; um front antigo não contorna a trava. O botão e a opção exibem o badge Beta. Salvar
  atualiza o chat do mesmo servidor sem exigir reload; desligar durante uma chamada desmonta o
  componente e encerra áudio/WebRTC pelo teardown existente.
  o botão Voz usa a conta e o app-server da sessão aberta. A conversa roda numa thread efêmera
  organizadora; a thread de trabalho só recebe o pedido consolidado depois da confirmação.
  WebRTC com `version: "v3"` negociou com login ChatGPT Pro no CLI 0.154.0; o transporte WebSocket
  do Codex exigiu API key e o padrão WebRTC foi recusado por versão do protocolo. O WebSocket do
  **Hangar** só leva sinalização e mantém a posse da chamada (uma por sessão, heartbeat com prazo).
  `_consumir` continua sendo o único leitor das notifications; a chamada mantém um ouvinte de estado
  enquanto o SSE reconecta. O botão fica no compositor: o modal configura a chamada e fecha quando
  conecta, deixando o indicador "Em voz". Reabrir/fechar os controles não desliga a voz. Encerrar
  ou trocar de sessão libera áudio e envia realtime/stop,
  sem fechar o cliente compartilhado nem cancelar o turno do agente. Ditado fica bloqueado durante
  a chamada. A escolha fica em `cp_codex_voice`, por navegador/origem. O medidor usa RMS separado
  do microfone e do áudio recebido; anima apenas o span HTML da marca, nunca o SVG.
  Verificado com faixa silenciosa no navegador:
  ICE/DTLS conectados, silenciar desabilita a track, desmontagem encerra a track e o peer. A fala
  real foi exercitada na POC pelo usuário; interrupção nesta integração ainda requer teste falado.
  O app Expo ainda não tem esse controle.
  Encaminhamentos `<realtime_delegation>` são reconhecidos só na apresentação (`parseRealtimeDelegation`
  no core): cabeçalho "Conversa de voz", pedido no corpo e envelope integral nos detalhes. O texto
  original e seu ID continuam intactos; não usar `parsePeerMessage`, que também decide tráfego de pares.
  Só trocar o prompt não resolveu o envio de fragmentos. No CLI 0.154.0, `HandoffRequested`
  encaminha a transcrição da última fala antes de avisar o cliente. O organizador em
  `codex_voice_broker.py` usa ferramentas dinâmicas para preparar/cancelar/confirmar um rascunho.
  A confirmação deve vir de outra interação, pelo `userMessage` real do app-server, e corresponder
  a uma autorização curta; o texto enviado é o rascunho imutável, não a confirmação. Revisão muda
  o ID; repetição não reenvia. A fila durável existente recebe o pedido, inclusive se o alvo estiver
  ocupado. Rascunhos ainda não enviados valem apenas durante a chamada. Shell, apps, hooks e MCPs
  ficam desabilitados no organizador; o catálogo foi conferido no request do CLI real com provedor
  local, sem modelo externo. O modelo é o da sessão, com esforço baixo, e há custo de organização
  na mesma conta. Contexto inicial usa a cauda já lida pelo Hangar, sem pedir o histórico inteiro
  pelo app-server. O organizador resume o resultado final e usa `appendSpeech`: `appendText` sozinho
  adicionava contexto, mas não produziu fala no teste. Teste com dois turnos de organização e
  entrada de áudio silenciosa confirmou pedido completo, resposta da sessão e retorno transcrito
  "A sessão respondeu: pinguim azul". Pausas e confirmações por áudio ainda exigem teste falado.

## Voz no app nativo

(`desktop-native/src/voice/`, `app/voice_ui.rs`, 07/10/2026): a voz roda no nativo, segue a sessão
aberta na tela (Claude ou Codex) e fala pela conta Codex desta máquina; backend e Python não mudam.
O nativo abre um `codex app-server` local (stdio, JSON-RPC em linhas; PATH do filho refeito por
`refreshed_path`, senão o atalho do npm/fnm sai sem `node`) e uma thread efêmera organizadora com
quatro ferramentas: `read_session` (a conversa que o nativo já tem em memória), `send_to_session`
(pela mesma rota do composer, na sessão da tela no instante do envio), `hold_request` e
`discard_request`. Envio direto; "espera/não manda ainda" segura o rascunho. O prompt sozinho não
segura fragmento (já falhou no web): todo envio espera 1,5 s e é cancelado se o usuário voltar a
falar; pedido com menos de 3 palavras é recusado. O transporte `websocket` do realtime foi recusado
com login ChatGPT no CLI 0.160.1 (`realtime conversation requires API key auth`), então o áudio é
WebRTC do próprio nativo: `str0m` 0.24.1 como ofertante + `opus-rs` 0.1.37, provado conectando em
1,5 s e recebendo fala (48 kHz mono, quadros de 20 ms); sem `Connected` em 10 s a chamada falha
(UDP bloqueado não dá erro, só não conecta). Eco: `sonora` 0.2.0 (AEC3 em Rust puro, o mesmo do
`codex-voice-host` da OpenAI), 36 dB de eco removido em sinal sintético com a voz local a −1,1 dB.
O `codex-voice-host` empacotado no Codex usa protocolo interno sem documentação; não é base.
O tempo de silêncio que encerra a fala é fixo no Codex (`server_vad`). `appendSpeech` parafraseia;
`appendText` sozinho não fala (aviso de troca de sessão vai por `appendSpeech`). Mídia antes do
`Connected` é descartada pelo str0m. Eventos da voz levam número de chamada e passam antes do filtro
de conexão do app: trocar de servidor não deixa ferramenta sem resposta. Resposta da sessão é
deduplicada por id de evento; sessão que recebeu pedido e saiu da tela tem a resposta lida pelo
histórico quando a lista mostra que ela parou. Gate: `codex_voice_beta` do servidor local
(loopback) e `codex` encontrado.

## Voz: modo Planejar

(`desktop-native/src/voice/organizer.rs`, 07/10/2026, Codex 0.160.1): a voz tem dois modos, trocados
pela chave do painel ou falando ("vamos planejar", "volta pro direto"). **Direto** é o de antes: o
pedido completo vai à sessão. **Planejar** não manda nada até o fim: o organizador escreve e
reorganiza um plano (objetivo, decisões, pendências, pesquisas com fontes) em
`~/.hangar/voz/planos/<sessão>-<AAAA-MM-DD-HHMM>.md`, gravado por tmp+rename atômico, e o painel
mostra o plano crescendo.

**Opção C.** O organizador pesquisa na internet (`web_search: "live"`), lê o projeto só para
leitura (thread em sandbox `read-only` com `approvalPolicy: never`) e pergunta à sessão o que só ela
sabe. Provado: `thread/start` aceita `web_search: "live"` (a pesquisa de fato não foi exercitada
na prova); com `environments: []` o modelo fica sem shell, então a chave saiu; sem ela, `ls` roda
sem pedir aprovação; escrever fora do cwd falha com "Read-only file system". Isso foi provado só no
Linux: no Windows `features.shell_tool` fica `false` até o sandbox ser provado lá, e o modo mantém
pesquisa na web e `ask_session`, sem leitura de código.

**Regras de segurança.**
- `finish_plan`, `ask_session` e `set_mode` só valem a partir de turno falado; a resposta da sessão
  que volta ao organizador não conta como fala.
- `finish_plan` tem dois passos: arma, e só confirma noutro turno falado. A voz lê o resumo e o
  usuário escolhe entre "leia e execute" e "escreva o plano de implementação a partir dele"; sai um
  único pedido apontando o arquivo.
- `send_to_session` e `hold_request` são recusados no Planejar.
- `ask_session` responde logo ao organizador; a resposta da sessão volta como turno marcado
  `[RESPOSTA DA SESSÃO À PERGUNTA]`, com prazo de 10 min, e não é falada como resultado.
- O plano é preso à sessão para a qual nasceu; sessão em outra máquina recebe o plano inline, já
  que o arquivo é local.

**Envio só após silêncio do microfone.** Só a espera fixa de 1,5 s gerou envio duplicado: duas
falas no mesmo turno, envios às 18:29:06 e 18:29:15. Agora o envio sai depois de 1,2 s de silêncio
do microfone, com teto de 8 s.

**RTP.** O pacote solto inicial que a OpenAI manda é descartado pelo `RtpStart`; a reserva de áudio
é de 240 ms porque o socket mostrou buracos de 66 a 190 ms com o laço rodando em 12 a 52 ms.

**O shell lê fora do projeto, e fica assim (decisão do Jefferson, 07/10).** Medido: `ls ~/.ssh` lista
10 entradas e `~/.codex/auth.json` é legível (conteúdo não impresso). A proteção é só a linha do
prompt ("leia só dentro da pasta do projeto; nunca abra credenciais"). Motivo: a sessão roda na
mesma máquina, com o mesmo acesso e com internet; restringir só o organizador não muda o risco.
Descartados: ferramentas de leitura restritas à pasta e desligar a pesquisa com o shell ligado.

- `adapters/kimi/` + `hooks/kimi_state_hook.py` + `kimi_hook_installer.py` — Kimi Code runs in the
  same tmux-native shape as Pi: TUI in the pane, chat from
  `~/.kimi-code/sessions/<wd>/<session_id>/agents/main/wire.jsonl`, state pushed by hooks in
  `~/.kimi-code/config.toml` (no pane scraping for state). The pane↔session link is the hook's
  ticket (`~/.claude/.hangar-kimi/<pane>.json`) — the CLI has no caller-chosen session-id.
- `sse.py` — merges the above into the SSE stream. `api.py` — FastAPI routes. `auth.py` — bearer token / `cp_token` cookie.
- Also: `pqueue.py` (durable input queue), `preview.py` (live in-flight block), `askquestion.py`
  (native AskUserQuestion stepper), `uploads.py`, `git_ops.py`, `commands.py`, `workflows.py`,
  `model_picker.py`, `config.py`, `fs.py`, `hook_installer.py`.

Frontend (`frontend/src/`): `screens/` (Chat, Board, …), `components/` (MessageList, NavBar, Composer,
bubbles, sheets, Spinner/Lottie, …), `lib/` (`api.ts` SSE client, `activity.ts`, `markdown.ts`,
`format.ts`, `types.ts`), `app.css` (design tokens + shared keyframes).

## Hook que falha: nem bloqueia, nem some (19/09/2026)

**O que aconteceu.** Uma sessão registrou um hook pessoal em `SessionStart`/`UserPromptSubmit`,
abriu uma sessão de teste (o que copiou o `settings.json` para a conta `claude-200-5`), depois
renomeou o script e corrigiu só o principal. A cópia ficou chamando `python3 <arquivo sumido>`,
que sai com código 2 — e código 2 em `UserPromptSubmit` barra o prompt. As três sessões da conta
pararam de receber mensagem. O Claude Code mostrou o erro no terminal e o gravou no transcript
como entrada `system`; o Hangar só transformava essa entrada em bolha quando o texto era recado
(`[de: …]`), então a fala da pessoa sumia do chat, e sem terminal o erro não aparecia em lugar
nenhum. Achar a causa dependeu de outra sessão ler o `settings.json` da conta.

**O que mudou.**
- `transcript._blocked_prompt`: todo prompt barrado vira bolha `held:` com `desistiu=True` e o
  texto do hook em `hook_error`. Conferido no transcript real: 4 falas, 9 entradas (os reenvios).
- `pqueue.merged_history` junta as entradas de mesmo id `held:` — o SSE já juntava por id, a
  lista do histórico não, e a tela mostrava 9 bolhas para 4 falas.
- `pqueue.historico_etag` leva a data do código do backend: com o validador só por metadado do
  arquivo, o `304` segurava no cliente o histórico lido pelo parser antigo enquanto a conversa
  não andasse. Medido na mesma sessão: backend devolvendo 4, tela mostrando 9 até o ETag mudar.
- `hook_installer._falha_avisa`: nos dois eventos cujo stdout entra no contexto, `|| exit 0`
  virou `|| echo "<aviso>"`. Provado em `sh`, `bash` e `fish`: script presente sai 0 sem aviso,
  script sumido sai 0 com aviso. JSON em aspas simples foi descartado por não rodar no `cmd`.

**O que o aviso NÃO pega.** Falha engolida dentro do script (`state_hook.py` e `nav_hook.py`
embrulham tudo em `except` e saem com 0). O aviso cobre script sumido, Python sumido e crash.

## Progresso de MCP no Claude sem terminal

Em 23/09/2026 o usuário notou que, sem terminal, uma chamada longa de MCP (o `objetivo` do
`hangar-computer-control`, 7 min) só mostrava "Executando…". A suspeita era o adapter perder o
progresso; não perde. Prova com um MCP descartável que chama `report_progress` três vezes, rodado
no `claude -p` 2.1.280 com as mesmas opções do adapter (`stream-json` nos dois sentidos,
`--verbose`, `--include-partial-messages`): o Claude pede o progresso (`progress_token` e
`claudecode/toolUseId` no `_meta`), o MCP manda, e o stdout traz zero `tool_progress`. No binário,
o conversor para o stream repassa `bash_progress`, `tool_heartbeat` (só segundos decorridos),
`repl_tool_call` e `agent_api_retry`; `mcp_progress` fica só na TUI. Também não entra no `.jsonl`
da conversa. O `bash_progress` também não sai no stream: o conversor só o repassa com
`CLAUDE_CODE_REMOTE` ou `CLAUDE_CODE_CONTAINER_ID` no ambiente.

A saída parcial do Bash existe em disco: o Claude Code redireciona o comando para
`<tmp>/claude-<uid>/<pasta>/<sessão>/tasks/<id>.output` (no Windows,
`%TEMP%\claude\<pasta>\<sessão>\tasks\<id>.output`) e apaga o arquivo no fim. O nome não traz o id
da chamada, então `procinfo.saida_de_comando` liga pelo processo: a saída dele aponta para o
arquivo e a linha de comando traz o comando no `eval '…' < /dev/null` (o `_comando_pedido` de
sempre). Comando igual ao do cartão = arquivo certo. Vale com e sem terminal. No Linux lê
`/proc/<pid>/fd/1`; fora dele, `psutil.open_files()` só no processo que casou pela linha de
comando — medido na VM Windows (Git Bash): 7–57 ms por leitura, saída ao vivo linha a linha.
Na DELPHI-02 o `%TEMP%` é curto (`ADMINI~1`) e o Claude põe o sufixo do comando entre aspas
(`pwd -P >| '/c/Users/ADMINI~1/…'`); o `_comando_pedido` cortava na última aspa da linha e
levava o sufixo junto, então nada casava. Agora ele lê o `eval '…'` como string de shell (aspa
fecha; `'"'"'` e `'\''` são aspa escapada). Medido lá depois da troca: 10 leituras ao vivo, ~40 ms.

Por isso o canal é um arquivo por chamada, gravado pelo próprio MCP, com o `toolUseId` validado
por regex dos dois lados (é nome de arquivo). O `hangar-computer-control` foi o primeiro a gravar;
qualquer MCP nosso pode seguir o mesmo formato. Os arquivos não são apagados: são poucos bytes por
chamada. Se a pasta crescer a ponto de pesar, a faxina é apagar os mais velhos que alguns dias.

## Provider `orq`: a linha do orquestrador não tem pane

28/09/2026 (spec `2026-09-28-orquestrar-auto-design.md`, §3). O orquestrador da `orquestrar-auto`
é o `orq advance`, um programa disparado pelo vigia e pelo `orq commit`: não há tmux, sidecar de
sessão nem processo vivo por trás da linha. Sem a recusa, `/input`, `DELETE`, `rename` e
`interrupt` caíam no caminho do tmux de uma sessão que não existe, e o pareamento gravava um
sidecar para um nome que nunca responde. A recusa vive num ponto só (`_recusa_orq`, que consulta
`runs.find`), e o 409 traz a frase "fale com o árbitro", porque é ele quem decide pela execução.
O estado da linha sai da atividade: trabalhando com `advance.lock` preso (lido em `/proc/locks`,
sem pegar a trava) ou com a linha do tempo/trava mexida nos últimos 2 min.


### Deltas Claude/Codex: acumular antes de publicar

**03/10/2026 — parte 2A.** Base Python `aa8eec36`, consumidores finais `7149f6d9`;
Linux/CachyOS, CPython 3.14.6, mesma máquina e venv preexistente. Sem serviço, instalação,
credencial ou CLI real. Nenhum runtime foi portado para Rust nesta entrega; protocolo e cano
continuam na versão 1.

O buffer compartilhado usa StringIO, publica primeiro imediatamente e coalesce intermediários
em 150 ms. Timer entrega a cauda sem outro delta; flush entrega o último no fim de bloco.
Limpeza cancela timers e aguarda publicação em voo. Claude mantém texto, pensamento e input
de ferramenta separados; o input parcial e seu rótulo continuam visíveis até o transcript.
`_input_parcial` permanece idêntico à base. Codex resolve a fonte na publicação e separa
preâmbulo/resposta. Estado, uso, pergunta, permissão e fim de turno/drain conservam o caminho
imediato. A limpeza de uma sessão antiga reconfere a identidade depois de esperar o descarte,
para não apagar os canais de outra sessão criada com o mesmo nome.

**Método.** Consumidores Claude reais `_Sessao`/`_on_stream` da base e da entrega, carregados
no mesmo interpretador; eventos stream-json sintéticos. JSON compacto UTF-8: Write com
`file_path=/tmp/ação-😀\synthetic` e content ASCII de 64/128/200 KiB, ou campo prompt ASCII de
200 KiB. Pedaços medidos em bytes, decoder incremental preservando Unicode. Três repetições
por combinação; as tabelas mostram medianas. Rajada sem espera entre eventos, usando o relógio
normal do loop, sem avançá-lo artificialmente. Cada execução confirma o input final completo;
a instrumentação retém só primeiro/último texto e conta parse/publicação. O frame `{}` do
início da ferramenta fica fora das contagens abaixo. Sem assinante e com uma assinatura real
de PushPreviewSource, sem HTTP/SSE/rede. Assinante pode observar só o último frame da rajada:
isso é a semântica existente de substituição completa, não perda de conteúdo final.

Parede: perf_counter do primeiro delta até block-stop. CPU: process_time no mesmo trecho,
incluindo parser/serializer/push/rótulo e instrumentação, sem filhos. Maior publicação: máximo
de cada execução e mediana desses máximos; antes mede o delta inteiro (concatenação, parser,
push e rótulo), depois o callback de publicação (parser, push e rótulo). A diferença de escopo
é explícita: não comparar esses máximos como operações idênticas. Atraso do loop: heartbeat
com sleep de 1 ms, maior excesso sobre esse prazo, mediana dos máximos. Não é CPU do backend
vivo nem de vários aparelhos. Primeiro frame permaneceu imediato nos dois caminhos.

**Rajadas — antes → depois, tempos em ms.** Parse e publicação têm a mesma contagem em cada
linha. O conjunto contém 60 execuções (cinco inputs × dois modos de assinatura × duas versões
× três repetições), com conteúdo final exato em todas.

| Assinante | Input / pedaço | Bytes / pedaços | Parede | CPU | Parse/publicações | Maior publicação | Atraso do loop |
|---|---|---:|---:|---:|---:|---:|---:|
| não | Write 64 KiB / 128 B | 65592 / 513 | 19,602 → 2,049 | 19,552 → 2,041 | 513 → 2 | 0,148 → 0,260 | 18,712 → 1,245 |
| não | Write 128 KiB / 128 B | 131128 / 1025 | 73,710 → 1,724 | 73,578 → 1,718 | 1025 → 2 | 0,319 → 0,268 | 72,911 → 0,910 |
| não | Write 200 KiB / 128 B | 204856 / 1601 | 163,671 → 2,709 | 163,305 → 2,697 | 1601 → 2 | 0,473 → 0,435 | 162,970 → 2,067 |
| não | prompt 200 KiB / 128 B | 204813 / 1601 | 5487,248 → 2,116 | 5473,232 → 2,108 | 1601 → 2 | 10,218 → 0,444 | 5486,547 → 1,364 |
| não | prompt 200 KiB / 32 B | 204813 / 6401 | 21994,111 → 6,758 | 21947,474 → 6,746 | 6401 → 2 | 11,653 → 0,456 | 21993,396 → 6,026 |
| sim | Write 64 KiB / 128 B | 65592 / 513 | 18,753 → 0,769 | 18,726 → 0,765 | 513 → 2 | 0,141 → 0,152 | 17,866 → 0,938 |
| sim | Write 128 KiB / 128 B | 131128 / 1025 | 66,450 → 1,464 | 66,351 → 1,458 | 1025 → 2 | 0,277 → 0,278 | 65,650 → 0,639 |
| sim | Write 200 KiB / 128 B | 204856 / 1601 | 165,035 → 2,232 | 164,725 → 2,224 | 1601 → 2 | 0,448 → 0,411 | 164,340 → 1,487 |
| sim | prompt 200 KiB / 128 B | 204813 / 1601 | 5508,786 → 2,160 | 5495,128 → 2,155 | 1601 → 2 | 9,971 → 0,448 | 5508,332 → 1,432 |
| sim | prompt 200 KiB / 32 B | 204813 / 6401 | 21852,820 → 6,821 | 21801,570 → 6,812 | 6401 → 2 | 10,908 → 0,451 | 21852,125 → 6,090 |


**Fluxo espaçado.** Prompt de 1 KiB, envelope de 1037 bytes, nove pedaços de 128 B, chegada
nominal a cada 40 ms, produção com intervalo de 150 ms. Depois do último delta, o consumidor
novo espera o timer completar a cauda antes de enviar block-stop: o EOS não fabrica essa prova.
Doze execuções (dois modos × duas versões × três repetições), input final exato em todas.
Tempos abaixo são medianas; instantes da prévia são de uma repetição representativa (a segunda).
Parede inclui as esperas de chegada/cauda; a janela da cauda pode aumentar a parede apesar
de reduzir o processamento. Não apresentar esse tempo como latência de controle.
Neste input pequeno a CPU total não caiu; a conta inclui temporizadores e heartbeat durante
a espera mais longa. A cauda é aguardada por Event, sem polling de conteúdo. Não extrapolar
a redução de publicações da rajada para ganho uniforme de CPU em fluxos pequenos.

| Assinante | Parede antes → depois (ms) | CPU antes → depois (ms) | Publicações antes → depois | Instantes depois (ms) | Atraso do loop antes → depois (ms) |
|---|---:|---:|---:|---|---:|
| não | 366,920 → 452,223 | 8,395 → 8,523 | 9 → 4 | 0,042 / 150,635 / 301,181 / 452,175 | 0,221 → 0,180 |
| sim | 367,690 → 452,229 | 8,238 → 8,844 | 9 → 4 | 0,041 / 151,091 / 302,058 / 452,181 | 0,245 → 0,240 |


**Conferência e limites.** 200 testes focados passaram: test_stream_buffer,
test_claude_headless, test_codex_adapter e test_preview_push. Cobrem primeiro/periódico/cauda,
Unicode/escapes, publicação bloqueada, erro de timer, EOS/result/assistant, reset/interrupção,
EOF antigo durante substituição, rename de outra thread, fonte recriada por unsubscribe,
separação de agentMessage, geração substituída, preserve_preview e controles/permissão/pergunta/
drain com prévia pendente. Revisão independente apontou a limpeza pelo EOF antigo; regressão
falhou antes e passou após reconferir identidade nos três canais.

Uso real no app, dois aparelhos, Claude terminal, Codex nos dois modos com CLI real e Windows
não foram conferidos: a Task 4, Step 2 permanece pendente para o canal de testes do dono.
Nenhum serviço foi iniciado/reiniciado/parado. A medição cobre processamento Python sintético,
não transporte cano, API, UI, rede ou produção. A rajada reduz milhares de prefixos a primeiro
e final; o fluxo espaçado mantém atualizações intermediárias. Snapshot full-replace continua
custando seu tamanho. Não afirmar linearidade de todo o pipeline nem usar decode-final-only
como ganho equivalente de UI. A versão anterior também pode publicar em cada chegada quando
os pedaços são lentos; o ganho depende da cadência, tamanho e campo do input.

## Custo da observação terminal

(03/10/2026, Linux 7.1.3, i5-13400F, Python 3.14.6, tmux 3.7b, Rust release.) A 2C original
`d21a445b` fazia 12 comandos tmux e dois HTTP por captura; a grade analisava o mesmo texto
que o capture. O estado temporal foi devolvido ao Python, sem outro RPC; o observador usa
`no-output`, confere sessão/pane a cada rodada e captura uma vez, sem grade/checkpoint ANSI.
A captura e sua conferência mantêm as duas molduras de nonce: seis comandos tmux por rodada.

O maior excesso estava em `_http`: `build_opener()` incluía HTTPS e carregava certificados
por pedido, mesmo em HTTP loopback. Mil construções consumiram 3,386 ms CPU/construção;
cProfile de 100 chamadas atribuiu 0,315 de 0,354 s à criação do contexto HTTPS. O opener
final é reutilizado e monta somente os handlers HTTP, sem proxy ou redirect.

| Chats simultâneos | Python anterior, CPU/chat/rodada | 2C original, CPU/chat/rodada | Final, CPU/chat/rodada | Redução contra Python reexecutado |
|---:|---:|---:|---:|---:|
| 1 | 2,803 ms | 10,404 ms | 1,668 ms | 40,50% |
| 4 | 2,745 ms | 15,327 ms | 1,784 ms | 35,02% |

Método: quatro lotes independentes de 700 capturas/chat por caminho, ordem alternada,
20 rodadas de aquecimento excluídas; mesmos panes sintéticos 100×40, 180 linhas e spinner
congelado, mesmos classificador e valores de hook/plugin/sidecar. O `StateMonitor.stream`
Python e a rota Rust de produção rodaram completos; nenhum backend/provedor/conversa real.
Fixture HTTP de loopback com segredo sintético e tmux privado `-S`, configuração `/dev/null`.
O poll/cache foi acelerado para cobrar uma captura por rodada. Todos os lotes finais
exigiram zero fallback, exatamente um HTTP capture por rodada e paridade exata dos eventos.

CPU = delta `utime+stime` de `/proc` do Python produtor, Rust, servidor tmux, clientes
residentes e panes + `RUSAGE_CHILDREN` dos clientes tmux encerrados. Dividido por 700×chats;
ticks de 10 ms, lotes com segundos de CPU. Com um chat, Python/Rust/tmux finais custam
1,261/0,200/0,207 ms por captura; quatro chats, 1,347/0,229/0,207 ms. Clientes vivos/panes
ficaram abaixo de um tick. O Python anterior cria um processo por captura; final cria zero
durante o lote e conserva um cliente de controle por chat aberto, mais o Rust já existente.
A fixture inicia um Rust por lote, contabilizado separadamente.

A contagem separada do protocolo provou dez capturas = 63 comandos (três iniciais +
seis por captura), um cliente criado, attach `no-output`; CPU sem o wrapper de contagem.
Sessenta ciclos acquire+release custaram 2,000 ms CPU/ciclo (inclui filhos encerrados),
120 HTTP e 60 observadores; não entram nas médias contínuas. Amortizado em 700 rodadas,
é 1/700 criação de cliente por chat/rodada, contra um subprocesso a cada rodada Python.

Parede por rodada: um chat, Python 2,803 ms/final 1,655 ms; quatro chats, Python 4,804 ms/
final 5,072 ms. O ganho medido é de CPU e processos novos; a rodada concorrente ficou
0,268 ms mais longa. Intervalos entre os quatro lotes finais: 1,629–1,700 ms com um chat
e 1,779–1,789 ms/chat com quatro, sem sobreposição aos lotes Python. Redução =
1 − CPU final ÷ CPU Python reexecutado no mesmo ensaio.

Agregados por lote e fontes de reprodução ficam em
`.superpowers/sdd/2026-10-03-hangar-server-parte2c-revision/perf-baseline.md` e nos auxiliares
`perf-final-*.py` do mesmo diretório. Os snapshots/binários/socket privados foram temporários;
o relatório não contém conversa real. O cálculo puro Rust segue conferido pelas fixtures
Python. Remover a operação privada `reduce` sobe ambos os protocolos para 3; a coordenação
com `rust-parte2` reservou 4 ao contrato posterior da 2B.

## Velocidade de geração (tok/s)

O `tok_s` da faixa (média da sessão) somava os `output_tokens` e dividia pelo intervalo entre a
linha anterior do jsonl e a linha da resposta. O Claude Code só grava a resposta quando ela
termina, então esse intervalo inclui fila, leitura do contexto e o caminho entre o resultado da
ferramenta e o próximo pedido. Medido em 03/10/2026 nas sessões do hangar: espera média até a
primeira resposta de ~4,8 s, e o número ficava entre 60 e 80 tok/s na sessão inteira, sem mexer
(283 chamadas na `0c62a065`). Resposta curta, a maioria das chamadas de ferramenta, puxava o
número para baixo. Os tokens de subagente entravam na soma sem o tempo deles, puxando para cima.

Correção: subagente saiu do `tok_s`, e a velocidade "agora" e "últimas 10" passou a vir do
stream. O `output_tokens` do jsonl estava certo (igual em todas as linhas da mesma mensagem);
o problema era só o relógio.

Sem estimativa durante a geração: a primeira versão mostrava caracteres ÷ 4 enquanto a resposta
escrevia, e o número medido foi ~67 tok/s em voo contra ~122 exatos quando a mesma resposta
fechava. O pensamento chega resumido (`--thinking-display summarized`), então os caracteres
não acompanham os tokens. O "agora" é a última resposta fechada.

O relógio parte do `message_start`, não do primeiro pedaço de conteúdo. Medido no mesmo dia
com `claude -p --include-partial-messages --thinking-display summarized` (Opus 5.5):
`message_start` em 5,04 s, primeiro `thinking_delta` em 7,31 s, fim em 11,00 s com 431 tokens.
Partindo do primeiro pedaço daria 117 tok/s; partindo do `message_start`, 72. Os tokens do
pensamento foram gerados antes de o resumo dele chegar.

### Velocidade recente e primeira resposta do Codex (06/10/2026)

O acumulador Codex não preenchia as chamadas recentes. Passa a usar cada avanço do contador
oficial de saída e o tempo de modelo acumulado desde o avanço anterior; contador repetido não
cria chamada. "Agora" e "últimas 10" permanecem aproximados e incluem espera/processamento
inicial. Os totais e a média da sessão permanecem com o mesmo cálculo.

A primeira resposta é medida de `turn/started` até o primeiro `item/agentMessage/delta`
não vazio da mesma thread e turno, antes do agrupamento da prévia, nos adapters Python e Rust.
A média usa os turnos observados desde a conexão do consumidor; turnos anteriores conservam
a reserva do transcript quando não existe medida ao vivo. A identidade da mescla usa
`session_key`, porque o nome do rollout Codex inclui data e hora antes do UUID.
O evento privado `rate` ganhou a medida de primeira resposta; protocolo Python/Rust 36.

## Contexto e cota sem a statusline do Hangar (03/10/2026)

03/10/2026. Com a preferência da barra desligada, o `statusLine` é o da pessoa, num formato que o
app não lê, e os anéis de Contexto e da conta no rodapé do composer do nativo mostravam "sem
dado" o tempo todo, embora a informação existisse.

- **Contexto:** o `usage` da última resposta do agente principal no transcript é o pedido inteiro
  (`input + cache_read + cache_creation`). Subagente (`isSidechain`) e resposta `<synthetic>`
  ficam de fora. O id do modelo no transcript não diz se é a variante de 1M, então a janela sai
  do modelo da PRÓPRIA sessão (o que a statusline recebeu, o `--model` do processo ou o sidecar
  da sessão sem terminal) e só na falta dele do `model` do `settings.json` da conta: o Hangar abre
  a sessão com `--model opus[1m]` sem mexer nesse arquivo. Termina em `[1m]` ou o uso já passou
  de 200k (só cabe na de 1M) → 1M; senão 200k. `CLAUDE_CODE_MAX_CONTEXT_TOKENS` do processo (ou
  `context_window` do sidecar) vence os dois. Lido com o mesmo TTL da statusline e entra na
  assinatura da lista em baldes de 5%. O cache guarda o `jsonl` lido: depois do `/clear` o anel
  fica sem dado até a primeira resposta, em vez de mostrar o número da conversa anterior.
- **Cota:** o anel da conta usa a da API de uso, a mesma da pílula do topo, quando a linha não
  traz a janela de 5 h ou a semanal.
- **A linha do Hangar vence:** quando ela traz o contexto, o número dela é o exato
  (`context_window_size` do Claude Code) e continua sendo o usado.
- **Nome do modelo (04/10/2026):** com a barra própria, a pílula de modelo do composer mostrava
  só "Modelo" até a pessoa trocar pelo app, e voltava a isso a cada sessão nova. A lista passou a
  trazer `model`, o id em uso: o da última resposta do agente principal no transcript (sem a data
  do snapshot, e com `[1m]` quando o uso só cabe na janela de 1M ou o modelo configurado é a
  variante de 1M da mesma família), senão o da abertura da sessão
  (`--model` do processo ou o sidecar da sem terminal) e por fim o `model` do `settings.json` da
  conta. A resposta vem primeiro porque um `/model` digitado no terminal deixa o `--model` do
  processo velho. Sessão de motor não cai no `settings.json`, que guarda o modelo da Anthropic.
  Os parsers da linha (`parseStatusLine` do core e `status::parse` do nativo) usam o campo só
  quando a linha não traz o nome.

Medição (03/10/2026, sessão Claude com barra própria, Opus em `[1m]`): o transcript deu 539.351
tokens contra 489k a 510k da barra minutos antes (a conversa crescendo entre uma e outra); no
nativo os anéis passaram de "sem dado" para Contexto 54% e a conta 100% (semanal).

## Entrada terminal parada sem aviso

(05/10/2026, DELPHI-02, `hangar-server-parte1`.) A sugestão esmaecida lida como rascunho
(`docs/decisoes/windows.md`, "O psmux entende `capture-pane -e`") gerou 35 `composer_busy` em
54 s, uma linha `info` por tentativa, e o app mostrou "Na fila" sem motivo. O escritor devolve
`Deferred` com `cleanup: not_needed`, e `finalize_terminal` só conta tentativa quando a limpeza
foi provada: o teto de duas tentativas nunca chegava. A série agora espaça as tentativas e
aparece na tela depois de 30 s, sem desistir da entrada: rascunho do dono continua sendo do dono.

Na revisão do mesmo dia: com teto de 30 s e nada zerando a série, o dono que enviava um rascunho
longo deixava a mensagem do app parada até 30 s com o terminal livre, e a linha seguinte herdava a
espera de uma linha apagada. O teto caiu para 8 s, a série guarda o id da linha e, durante a
espera, uma captura só de leitura (`composer_free`) libera a tentativa quando o composer está
vazio. O lado Python só traduz `view.input_stalled`; não há espera espelhada lá.

## Reinício no meio de um adiamento

(05/10/2026, DELPHI-02.) Com o composer ocupado, cada tentativa abre despacho, lê a tela e devolve
`composer_busy` sem escrever nada. O Atualizar reiniciou o backend no meio de uma dessas
tentativas: o `Recover` do boot transformou o despacho em incerto, a trava de escrita segurou a
fila e a mensagem nunca mais saiu (sessões `Crack` às 09:09 e `Desktop` às 11:31). O despacho
sozinho não diz se algo chegou ao terminal; daí o estado `staged` até o escritor avisar a escrita.
Python antigo lendo `staged` continua conservador (incerta), e Rust antigo nunca grava `staged`:
nenhuma combinação reenvia o que foi escrito.

## Provedor padrão da abertura

Em 05/10/2026, Nova conversa, o modal e a criação de worktree com sessão começavam em Claude.
O mobile guardava a aba escolhida localmente, mesmo sem abrir uma sessão. CLI, MCP e API também
preenchiam Claude quando o chamador não especificava o provedor.

`last_session_provider` fica no `runtime-config.json` do servidor; `remember_provider` autoriza
gravá-lo após a criação. As interfaces enviam esse campo; CLI, MCP e API automatizada apenas
herdam o padrão. No CLI, `--remember-provider` permite gravá-lo explicitamente, sem a abertura
de auxiliares mudar a preferência humana. `/providers` marca `default` por item, usando o estado de login do Claude e do Codex já
existente, além dos motores configurados. Preferência indisponível ou sem conta conectada,
quando há outra conectada, cede ao provedor utilizável. Escolha explícita continua prevalecendo.

A abertura Codex sem conta explícita usa uma adicional conectada quando a padrão está
deslogada. Isso é escolha inicial por login, sem rotação por cota. Transferência com conta
explícita conserva a conta pedida.

Cobertura escrita em `test_session_defaults.py`, `test_api_providers.py`, `credenciais.test.ts`,
`newChatDraft.test.ts` e nos testes dos seletores: conta única GPT, último provedor, indisponibilidade, opção
explícita, criação recusada e erro de gravação da preferência. Os testes automatizados só rodam
quando solicitados.

Conferência em uso: build `release` do nativo, janela e servidor de dados isolados. Nova conversa
e o modal Nova sessão selecionaram Codex e uma conta adicional conectada com a padrão
deslogada. A abertura simulada com gravação explícita gravou o último provedor, e a reabertura do modal
refletiu Claude depois de uma criação Claude. A leitura do catálogo com contas reais também
foi exercitada, sem abrir sessão.

A PWA foi conferida em 393 × 852, formato de celular. A tela inicial `NewChatHome` tem seu
próprio carregador (`newChatDraft.svelte.ts`), que também precisa aplicar o padrão, bloquear o
envio durante a leitura e enviar `remember_provider`. No fluxo simulado, abriu Codex na conta
conectada; depois de criar por Claude, reabriu em Claude; retirado o login Claude, selecionou
Codex apesar da preferência anterior. Builds do nativo e da PWA passaram. O app Expo não foi
conferido em aparelho; os testes automatizados não foram executados.

A revisão de publicação acrescentou proteção para conta e retomada escolhidas durante a
leitura de `/providers`, além da confirmação do provedor já selecionado. Falha dessa leitura
aparece no mobile e requer uma escolha explícita. Avisos da gravação seguem pela resposta
de criação e do bastão até as interfaces; falha ao mostrar um aviso não torna a criação uma
falha nem provoca repetição. No nativo, a abertura por worktree também conserva esses avisos.

## /select do Codex sem terminal não passa pela rota do terminal Claude

Achado pela prova da parte 4 da migração Rust (Step 27, 06/10/2026): o cartão de aprovação do
Codex sem terminal (gpt-6-luna em "Ask for approval") aparecia na lista e no chat, e
`POST /select` respondia 500. Desde `bead8a454` (03/10) o `/select` chamava `route_sync` antes
de olhar o provedor; `route_sync` abre `prepare_session(name, "claude")`, que sem vínculo Claude
e sem pane levanta `RuntimeError("vínculo gerenciado indisponível; escrita suspensa")`. O ramo
do Codex sem terminal (`adapter.select`), que já respondia o cartão, nunca era alcançado.
Reproduzido em `test_select_on_headless_codex_answers_the_approval_without_the_claude_terminal_route`
com o coordenador real. O `/answer` já filtrava pelo provedor; nas outras rotas que passam por
`wrap_driver`, o Codex sem terminal é desviado antes (`/interrupt`) ou a rota é só de Claude/pane.

O erro que ainda escapar da rota no `/select` sai com código: `TerminalOutcomeUnknown` (a tecla
pode ter chegado) é 409 `erro_sem_confirmacao_resposta`, para ninguém repetir; o resto, anterior à
entrega, é 503 `erro_opcao_nao_convergiu`.

Prova real depois do conserto (`scripts/prova-parte4.py --casos 27`, backend isolado): Codex sem
terminal com cartão → `/select` 200 e a sessão sai do cartão; Claude sem terminal segue 200.

## Codex: retomada longa e envio sem confirmação

Em 08/10/2026, com Codex CLI 0.160.1, um restart do backend foi seguido por 338 reconexões e
45 falhas de confirmação de `turn/start`. O rollout continha 45 cópias da mesma mensagem de
usuário, cada uma recebida pelo Codex. A resposta de `thread/resume` inclui o histórico inteiro:
uma cópia isolada do rollout anterior ao restart, com 17 turnos, já devolvia mais de 16 MB.
O teto de 8.388.608 bytes do WebSocket encerrava a conexão; a exceção genérica virava `deferred`,
a fila liberava o claim e a mesma mensagem era enviada novamente.

A prova com o CLI real usa uma conta temporária sem credenciais, plugins desligados e uma
cópia do histórico anterior ao restart. Não inicia turno de modelo. O cliente da base encerrou
o WebSocket com código 1009 durante a retomada. O cliente corrigido recebeu 16.763.740 bytes,
recuperou os 17 turnos e manteve a conexão para um `thread/read` posterior. O limite do leitor
stdio continua separado; esta mudança remove o teto do WebSocket de loopback nos dois caminhos
de conexão, abertura e reconexão.

O pedido distingue conexão indisponível antes da escrita, recusa JSON-RPC e resultado incerto
após começar a escrita. A entrada é reservada antes do RPC e conserva o claim quando a resposta
se perde ou o envio é cancelado. A execução gerenciada registra `unknown`, usando o contrato
existente, e não repete a operação. A confirmação de Codex só confirma o que encontrou no
rollout, inclusive quando idle; ausência de texto não autoriza liberar a tentativa para reenvio.
Fechamento do WebSocket registra o código no diário, sem conteúdo da conversa.

As regressões cobrem resposta maior que 16 MB em WebSocket real, limpeza de pedido pendente,
distinção entre recusa e ausência de conexão, perda de resposta, cancelamento, confirmação
idle e persistência de operação incerta na execução gerenciada.

Conferência contra a base `e840c9e86`: seis casos novos falharam pelo motivo esperado, sem
erro de importação. Os dois transportes fecharam com 1009; os dois drains emitiram dois
`turn/start` em vez de um; a confirmação idle chamou o drain; a reserva declarou `accepted`
para um resultado incerto. Os mesmos casos passaram com a correção.

# Superado — mecanismos que não existem mais

Estes trechos descrevem como algo **funcionava antes**. Ficam registrados porque a
medição continua valendo como história, mas não descrevem o código de hoje: cada um
aponta a decisão que o substituiu. Não leia daqui para decidir implementação.

## `StateMonitor` e `PreviewBroker` de Claude com terminal com o Rust de pé

(Até a Task 5 da parte 4 da migração para Rust, 06/10/2026 → [estado ao vivo no `Monitor` do
Rust](plataforma.md#estado-ao-vivo-de-claude-com-terminal-no-monitor-do-rust).) Com o
`hangar-server` de pé, o `state` de Claude com terminal saía do `StateMonitor` do Python: o hub
assinava o `side-events`, o `merged_events(side=True)` rodava o monitor (captura alugada ao pool do
Rust por HTTP privado a 0,75 s) e o `PreviewBroker` (0,15 s trabalhando), seguia o transcript só para
suprimir a prévia já gravada, emitia `suggest` e `ask_question` no tique do estado e disparava o
`drain` na primeira borda entregável de cada conexão. Convidado (8766) e dono pelo Connect (8768)
subiam outro `merged_events` com o mesmo monitor compartilhado pelo `Difusor`. Hoje isso só roda no
modo `python`, e a ponte do observador que ele alugava (`terminal_observer` →
`/__hangar_server/terminal`) ficou sem consumidor: no modo `python` ela está desligada.

## Descoberta e lista do dono produzidas pelo Python com o Rust de pé

(Até a lista-estado, Tasks 13–17, 05/10/2026 → [lista do dono no
hangar-server](plataforma.md#lista-do-dono-no-hangar-server).) Com o `hangar-server` de pé, o
`_ListRefresher` do `sse.py` descobria as sessões e montava a lista a cada 1,5 s, e o
`registry.resolve_tracked` resolvia o transcript de cada uma no Python. Hoje a descoberta, a
resolução e a lista do dono são do `ListHub`; o `resolve_tracked` pergunta à ponte
(`list_bridge.resolve`), o `_ListRefresher` que sobra (lista do convidado) lê o retrato do Rust, e
a produção Python só roda no modo `python` (contador `PYTHON_DISCOVERY`).

## Passagem de sessão e de pedido entre Python e Rust com o Rust vivo

(Partes 2B–2D, PR #30 e 2C da migração para Rust, até 04/10/2026 → dono único,
`docs/migracao-rust/dono-unico/`.) Com o `hangar-server` de pé, uma sessão ou um pedido ainda
podiam trocar de dono no meio da vida:

- **Readoção a cada boot.** O lifespan registrava toda sessão sem terminal no Python e religava o
  cliente Python em todo cano vivo (`religou`); quando o Rust subia, `adopt_registered` adotava
  cada uma (`desligou`). A cada queda 1 ou 2 do Rust, `deactivate_runtime` fazia `recover` de todas
  para o Python e o Rust novo as adotava de volta.
- **Adoção com `quiesce`/`carry`.** O `adopt` passava a sessão por `PreparingRust`: `_peek` no cano,
  `LegacyBridge.quiesce` desligava o cliente Python, cancelava o drain, devolvia a reivindicação da
  fila (`drain_claims`, `runtime.unclaim_*`, `runtime.write_uncertain`) e montava o `carry` (modelo,
  esforço, comandos, uso) que o Rust aplicava por cima. Leitura e escrita da passagem tinham ramos
  próprios (`assert_legacy` `finishing`/`continuing`, `finish_wire(settling=...)`, `_SYNC` e
  `route_queue` lendo a vista Python, `owner_state_stream` esperando o novo dono com o diário
  `runtime.state_owner_stuck`).
- **Administração por `detach` → Python → `adopt`.** Renomear, parar, recarregar, trocar modo ou
  conta, transferir e o `run_admin` do terminal (`/model`, `/effort`, `/btw`, modo) devolviam a
  sessão ao Python, religavam o cliente dele e a adotavam de novo no fim; a parada do backend fazia
  `detach` + `quiesce` de toda sessão.
- **"Três tentativas + uma" e "só aquela sessão vai para o Python".** Uma operação recusada pelo
  Rust era repetida até 4 vezes com pausa de 2 s; esgotadas, `_hand_to_python` marcava a sessão
  `rust_refused` (diário `runtime.parte_para_python`) e ela ficava no Python até reiniciar. A
  entrega incerta do terminal também a passava.
- **Repasse das rotas públicas por falha.** O `Fallback` do Rust (`FALLBACK_AFTER = 4`) mandava
  histórico, eventos e Git/arquivos de uma sessão ao Python depois de 4 falhas, até reiniciar; o
  repasse levava o cabeçalho `x-hangar-workspace-fallback`, e a ponte Python de Git/arquivos rodava o
  próprio corpo quando o Rust devolvia vaga cheia, contexto quebrado ou indisponível
  (`workspace.reserva_python`).
- **Observação do terminal com reserva Python por erro.** Captura do Rust que falhava caía no
  `tmux capture-pane` do Python, com um disjuntor por sessão (3 falhas, pausa de 1 a 30 s).

O código de passagem foi a origem da maioria dos defeitos de 04/10 (primeira mensagem sumindo,
"sessão em transferência", entrega marcada sem chegar, reserva circular de Git). Hoje o dono é
decidido por processo, plataforma, provedor ou tipo de pedido, nunca por uma falha: com o Rust de pé,
falha vira erro com código e motivo, a sessão sem terminal nasce e reabre direto nele, a
administração fecha e reabre no Rust, o terminal empresta o teclado ao Python por uma operação, e o
Python só atende o que migrou quando é dono da porta inteira. Regras em
[plataforma.md](plataforma.md#hangar-server-a-porta-pública-em-rust-o-python-atrás) e o desenho em
`docs/migracao-rust/dono-unico/desenho.md`.

## `termsock` como dono do PTY com o Rust de pé

(Até a parte 4, Task 9, 06/10/2026 → `plataforma.md`, "Porteiro do terminal".) Com o
`hangar-server` de pé, o `termsock` abria o PTY do convidado e do dono pelo Connect no Python
(`_motor_posix`), enquanto o dono na 8765 já tinha o PTY no Rust: dois donos do "um painel por
sessão" (`termsock._ativos` e o `Terms` do Rust), e o 409 e o `terminal_panel` do `/api/config`
só enxergavam o do Python. Os motores do Python ficaram só para o modo `python`.

## Aviso de reinício do atualizador às sessões

(`atualizar._avisar_sessoes`, 25/08/2026 → removido em 24/09/2026). Antes de reiniciar, o botão
Atualizar rodava `hangar-send --group "[hangar] o backend vai reiniciar…"` para "as sessões vivas".
O atualizador não é uma sessão: o `hangar-send` caía na sessão do cliente tmux anexado. Com ela
fora de grupo o log mostrava `404 erro_sessao_sem_grupo`; com ela num grupo, o aviso ia só para
aquele grupo, assinado como se ela tivesse mandado. Nunca chegou a todas as sessões. As sessões
seguem vivas pelo reinício e o app reconecta sozinho, então o aviso saiu em vez de virar um
broadcast que custaria uma resposta de cada sessão por atualização.

## Login do ChatGPT (Codex) é UM login pra três CLIs, e quem faz é o app

(`app/oauth_codex.py` +
  a linha "Conta do ChatGPT (Codex)" do `NovaCredencialSheet` + `app/harness_saude.py`, 04/09/2026).
  Codex CLI, Pi e omp usam o MESMO OAuth — `client_id app_EMoamEEZ73f0CkXaXp7hrann`, mesmo
  `auth.openai.com/oauth/token`, mesmo fluxo de código de dispositivo — e cada um guarda o resultado
  no formato dele: `~/.codex/auth.json` (`tokens.{id_token,access_token,refresh_token,account_id}`),
  `~/.pi/agent/auth.json` (`openai-codex: {type:"oauth", access, refresh, expires em ms, accountId}`)
  e a tabela `auth_credentials` do `~/.omp/agent/agent.db` (`provider`, `credential_type='oauth'`,
  `data` = a credencial do Pi sem o `type`, `identity_key` = accountId; conferido com
  `omp token openai-codex` devolvendo JWT). O app roda o fluxo de dispositivo sozinho (stdlib), guarda
  em `~/.hangar/auth/openai-codex.json` (0600) e escreve nos três — **só onde não há login**; login
  existente é da pessoa. Três medições que decidem o desenho:
  - **Rotação do refresh não invalida a cópia.** Renovando pelo refresh do Codex por fora, a resposta
    trouxe refresh NOVO, o antigo continuou renovando e o Codex seguiu autenticando com o store
    intocado (o 400 seguinte foi de modelo, não de auth). Por isso cada CLI renova sozinho, sem
    renovador central; o custo de um dia isso mudar é a linha "Login" do painel ficar vermelha.
  - **A Cloudflare do `auth.openai.com` devolve 530 (`cf_route_error`) pro User-Agent padrão do
    urllib.** Qualquer outro UA passa; `_http` manda `hangar/1.0`.
  - **`earliest_refresh_at` na resposta do token**: o servidor diz quando o próximo refresh é aceito
    (~9 dias, com o access valendo 10). Não é erro, é o ritmo dele.
  O `codex login` (0.153.1) não tem `--device-auth` visível no `--help`; o app não depende dele.

## Proibição de instalar o Windows como administrador

Em 10/09/2026, o instalador passou a recusar execução elevada: tarefas criadas com dono
Administradores impediam que a atualização comum usasse `Register-ScheduledTask -Force`.
Em 12/09/2026, a proibição foi substituída por níveis coerentes: uma instalação elevada
registra tarefas interativas `Highest` e atalhos elevados; a atualização herda o backend.
O problema medido era misturar permissões, não uma incapacidade do psmux elevado de colar.
Regra atual em [instalacao.md](instalacao.md#instalação-windows-mantém-o-nível-de-permissão).

## Lançador Windows sem acompanhamento e vigia baseada só em porta

O `.vbs` usava `Run(..., 0, False)`: a tarefa terminava enquanto o Python continuava vivo.
A vigia consultava porta TCP e idade dos processos; após dez minutos sem porta, iniciava
outra instância sem encerrar a anterior. Em 12/09/2026, esse fluxo foi substituído por tarefa
que aguarda o filho, recuperação nativa e vigia HTTP com parada seletiva. Evidência e limites
da validação em [instalacao.md](instalacao.md#a-tarefa-windows-acompanha-o-processo-até-ele-terminar).

## O jev-gateway como caminho da conversa

Entre 19 e 20/09/2026 o Hangar sabia pôr uma sessão atrás do `jev-gateway` (projeto externo,
checkout em `~/.local/share/jev-gateway`): `ANTHROPIC_BASE_URL` no Claude, `model_provider` por
`-c` no Codex sem terminal, campo `jev_gateway` na criação, `--jev-gateway` no `hangar-send`,
caixa na folha e padrão por servidor. O gateway perguntava ao Jev qual tool o turno pedia e, com
confiança, forçava `tool_choice` (Codex) ou sugeria (`hint`, no Claude, porque raciocínio ligado e
cache impedem forçar).

Saiu em 20/09/2026 por decisão do Jefferson, e o motivo é de desenho, não de medição: para
escolher UMA tool, a conversa inteira do turno passava por um terceiro. Preço desproporcional ao
que se ganhava — e o que se ganhava, medido, era pouco ou negativo:

- **Codex** (0.154.0, `gpt-5.6-sol` high, criação de tela, uma execução por lado): saída +0,9%,
  entrada −11,8%, tempo +8%. As tools vêm embrulhadas numa `exec` de JavaScript, então o gateway
  via 3 tools de topo e a escolha real acontecia dentro do script, fora do alcance dele.
- **Claude** (Claude Code 2.1, `claude-fable-5-1` high, depuração com 7 bugs plantados e 16 testes,
  uma execução por lado, tokens somados do `usage` por `message.id`): 17 pedidos ao modelo contra
  9, entrada 1.082.854 contra 630.157 (**+72%**), saída 3.671 contra 3.158 (+16%), 2min53 contra
  55s. Os dois lados fecharam 16/16. Uma execução por lado não separa isso de variação entre
  rodadas — era por isso que a opção nascia desligada.

Duas coisas que a remoção também leva embora: a sonda de porta antes de subir a sessão (provedor
apontando para porta fechada é sessão que nasce e nunca fala com o modelo) e a degradação calada
no relançamento, que trocava o regime de custo sem nada na tela dizer em qual regime a sessão
estava. Ficou por conferir, e agora não será: `thread/resume` de uma thread criada com o gateway
numa subida sem ele.

O `jev` da sessão CONTINUA — é outra coisa: só põe a chave da TypeSafe no ambiente, para o
`hangar-preview objetivo` navegar sozinho. Regra atual em
[harnesses.md](harnesses.md#regras-vigentes).

## Function hooks fora do Hangar

Em 14/09/2026 os function hooks do Claude Code foram medidos e deixados de fora: a API é de
acesso antecipado e muda sem aviso, e o interruptor `claude_function_hooks` só punha a variável
no ambiente para o plugin de quem usa o Hangar. Em 18/09/2026 o Hangar passou a carregar um
plugin próprio (`plugins/hangar`) atrás do mesmo interruptor, como caminho opcional por cima do
tmux: o risco da API virou fallback por ausência, não motivo para não usar. Regra atual e
medições em [harnesses.md](harnesses.md#function-hooks-o-plugin-pluginshangar-é-um-plus-por-cima-do-tmux-18092026).

## Codex sem terminal no Python (decisão 2 do dono único)

(Até a parte 5B da migração para Rust → [Codex sem terminal: o cano é do
Rust](harnesses.md#regras-vigentes), `docs/migracao-rust/parte5-codex/spec.md`.) A decisão 2 de
`dono-unico/desenho.md` deixava o Codex sem terminal no Python, como provedor não migrado, até
haver prova real do Codex no Rust: o dono era fixo pelo provedor, o `matar_orfaos` do Python
varria os canos e o `-32601` valia para todo pedido do servidor sem tela. Hoje o Rust sobe, religa
e mata o cano, responde os pedidos (o `-32601` ficou só para os sem tela) e atende as rotas só do Codex; o Python faz isso apenas no modo
`python` (Rust ausente). Codex com terminal continua no Python até a 5C.

## Voz Codex no web, por sessão

(10/09/2026 até 10/10/2026 → [voz ao vivo no
hangar-server](harnesses.md#voz-ao-vivo-no-hangar-server-10102026).) `codex_voice.py`,
`codex_voice_broker.py`, `CodexVoice.svelte` e `lib/codexVoice.ts`: o botão **Voz · Beta** no
compositor de uma sessão Codex usava a conta e o `app-server` daquela sessão, com uma chamada por
sessão. O WebSocket do Hangar só levava sinalização e a posse da chamada (heartbeat com prazo). A
conversa rodava numa thread efêmera organizadora; a thread de trabalho só recebia o pedido
consolidado depois de confirmação noutra interação (rascunho imutável, revisão mudava o ID,
repetição não reenviava, entrega pela fila durável). Shell, apps, hooks e MCPs ficavam desligados
no organizador; modelo da sessão, esforço baixo. A voz escolhida ficava em `cp_codex_voice`, por
navegador.

Medições que continuam valendo como história: no CLI 0.154.0, WebRTC com `version: "v3"` negociou
com login ChatGPT Pro, o transporte WebSocket do Codex exigiu API key e o WebRTC padrão foi
recusado por versão do protocolo; `HandoffRequested` encaminhava a transcrição da última fala antes
de avisar o cliente; só trocar o prompt não resolveu o envio de fragmentos; `appendText` sozinho
adicionava contexto, mas não falava (o organizador passou a usar `appendSpeech`). Verificado com
faixa silenciosa no navegador: ICE/DTLS conectados, silenciar desabilitava a track, desmontar
encerrava track e peer; dois turnos de organização com entrada silenciosa devolveram "A sessão
respondeu: pinguim azul". Saiu porque a voz passou a ser uma só por servidor, seguindo a tela em
vez de presa a uma sessão Codex.

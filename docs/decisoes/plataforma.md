# Plataforma — nomes, pareamento, planos, ditado

Decisões medidas, com data e número. O `CLAUDE.md` carrega a regra;
a medição que a sustenta mora aqui. Conteúdo movido sem alteração.

## Revisão de código

neste repositório GitHub, usar revisão local e as verificações
  do projeto. A instalação local do CodeRabbit pertence a outros repositórios.

## Diário de uso: causa e contexto no arquivo exportado

Em 12/09/2026, um diário Windows de 06–12/09 tinha 19 registros de `mux.indisponivel`,
24 de falha da listagem e três envios incompletos. Contagem por evento do JSONL, não por
incidente: o mesmo problema pode aparecer em mais de uma camada. Os motivos detalhados
estavam só no log local, que não acompanha o download.

`tmux._run` registra falhas e comandos que demoram pelo menos um segundo: operação conhecida,
retorno, classe/errno/winerror, prazo, duração, comandos simultâneos ao iniciar e memória
total/disponível naquele instante. Ausência normal de sessão não vira erro. `envio.parcial`
acrescenta etapa, tamanho/linhas, geometria do campo, contagem de colagens e resultado da limpeza;
`envio.clipboard` distingue sessão Windows incompatível, escrita e tecla de colagem.

`api.servidor` mede até os cabeçalhos da resposta, não a duração de streams: ações, erros e
leituras lentas. Usa o template da rota, sem query nem corpo. O pool dedicado de envio propaga
o `req` com `copy_context`, ligando tela, resposta e comando. Cada evento do backend identifica
versão, PID e início do processo; o cabeçalho traz ambiente e versão do CLI consultados no download.
Isso não reconstrói informações ausentes de diários antigos nem identifica a versão de um
servidor psmux já aberto antes de trocar o binário.

Nunca copiar argv, stdout, stderr ou `_diag_composer` para esses eventos: podem conter conversas
e credenciais. Os testes em `test_diag_runtime.py` conferem o JSONL exportado, incluindo ausência
de conteúdo sensível, concorrência, correlação e falhas Windows simuladas.

A ampliação no mesmo dia cobre login/autorização Claude e Codex, cadastro, exclusão,
reconciliação e preparação de contas em segundo plano, autenticação do app, sync, peers,
criação de sessões e ciclo do SSE. `req` liga requests; `operacao` liga etapas, inclusive
entre pedidos de uma tentativa de login. `@diag.rastrear` registra início e término sem ler
argumentos/retorno: `retornou` significa que a função terminou, não que um resultado parcial
virou sucesso. Erros encadeados preservam classe/errno/winerror sem copiar suas mensagens.

Conexões SSE levam o mesmo `req` da tela na query `diag_req`, validada só nas rotas de eventos;
o header continua tendo prioridade. Web, comparação e mobile preservam autenticação e posição
de retomada. A lista registra abertura/fechamento por conexão. O refresher da lista e as fontes
do `Difusor` limpam o `req` herdado para não atribuir trabalho compartilhado ao primeiro ouvinte.
Os testes focados cobrem duas conexões simultâneas com IDs distintos e produtores sem esse ID.

Logs ficam sob `%LOCALAPPDATA%/hangar/logs` no Windows e `~/.hangar/logs` no Linux/macOS.
`diario/` contém somente o JSONL exportável; `privado/` recebe backend, instalador, vigia,
hooks, restauração de terminais e diagnósticos do CLI. O backend usa rotação de 4 MiB com
três backups. Os avisos/erros de `hangar`, `app`, `uvicorn.error` e `asyncio` acrescentam ao
diário apenas arquivo/função/linha e tipo da exceção. O setup roda também no lifespan, pois
a configuração do uvicorn substitui seus handlers depois do início do processo principal.
O atualizador destacado usa `atualizacao.log`, para não disputar a rotação do backend.
O shell Electron escreve em `privado/shell.log` (`shell/log.cjs`, uma rotação de 4 MiB na
subida): lançado pelo `.desktop` ou pela tarefa do Windows ele nasce com stdout/stderr em
`/dev/null`, e os `[nav]` que dizem por que uma aba do navegador embutido não congelou ou não
descongelou morriam sem leitura — foi o que faltou ao investigar um "ficou branco" que não se
reproduziu depois.

Os diários antigos são copiados por origem para `diario/legado`, sem juntar arquivos de
mesmo nome nem apagar originais; o download atualiza a cópia se uma versão antiga ainda
escreveu lá. Logs privados antigos conhecidos têm a cauda de até 4 MiB copiada na subida;
o original completo permanece. Instaladores atualizam os destinos dos lançadores; reiniciar
só o Python não muda o redirecionamento de stdout de um lançador Windows antigo.

Web e Expo usam o transporte de diagnóstico do core. A fila limitada fica em memória,
separada por destino, e remove o lote somente após resposta HTTP de sucesso. As reconexões
registram motivo, espera e recuperação; respostas/códigos e falhas de parse não incluem
payload. Fechar/recarregar o app antes da entrega ainda perde a fila local; ela não é um
armazenamento persistente. O servidor registra por conta própria as recusas que recebeu.

## Comentário explica o PORQUÊ, e é curto. A história medida mora AQUI, não no código.

Este
  arquivo é longo de propósito: é o lugar onde decisão medida, com data e número, sobrevive e é
  relida. O código não é. Lá vale a regra, não a arqueologia dela: se o comentário repete o que o
  nome da função já diz, sai; se a explicação ficou maior que o trecho que ela explica, o excedente
  vira linha de commit ou entrada em `docs/decisoes/`. **Medição, versão de CLI e data envelhecem** — num
  comentário elas viram afirmação falsa que ninguém revisa, enquanto aqui e na mensagem de commit
  estão datadas por construção. Isso não é licença pra código mudo: o porquê não-óbvio continua
  obrigatório, em uma ou duas linhas.

## O nome antigo (`claude-pocket`) só existe em ponte de compatibilidade

(rename de 25/08/2026).
  O código conhece **um** nome: as pastas de dados são `<config>/.hangar-*`, o cofre do sync é
  `~/.hangar/`, os comandos são `hangar-send`/`hangar-engine`/`hangar-codex`/`hangar-conta` e
  `hangar-panel-*`, a instância do quickshell é `hangar`, e o logger é `hangar.*`. Escreva com o
  nome novo; nunca leia o antigo em código novo. O que resta do velho é, todo ele, migração:
  - `backend/app/migracao_sidecars.py`, chamado na **subida** do backend (`main.py`), renomeia
    `.claude-pocket-*` → `.hangar-*` em todo perfil `~/.claude*` e deixa **link** no caminho antigo.
    O link é o que impede a máquina de se partir no meio da atualização: hook, extensão do Pi e o
    publicador de statusline do Kimi (`~/.kimi-code/statusline.js`, que nem mora neste repo) podem
    estar vivos e desatualizados, escrevendo no nome velho — e caem na pasta nova. Startup, e não
    installer, porque atualizar é `git pull` + reiniciar o serviço; rodar `install-*.sh` não é
    garantido. Ele **nunca funde** duas pastas: destino já existente para naquele item, com aviso.
  - `.json` SOLTOS (`apelidos`, `conn`, `models`, `runner`, `opencode`) leem os dois caminhos, novo
    primeiro (`migracao_sidecars.caminho_de_leitura`), porque no Windows link de ARQUIVO exige
    privilégio — para pasta há junção (`mklink /J`), para arquivo não há equivalente. **Escrita
    sempre no nome novo.**
  - Anexo e diário de orquestração **saíram do projeto e do config dir da conta** (31/08/2026) e
    moram no cofre: `~/.hangar/uploads/<projeto>/<sessão>/` (`uploads._base()`) e `~/.hangar/orq/`
    (`orq.raiz_padrao()`). Os dois estavam no lugar errado pelo mesmo motivo — um lugar que
    pertence a *outra coisa*. O anexo nascia dentro do repositório trabalhado, e o `.gitignore` que
    o esconde é o DESTE projeto: em qualquer outro repositório ele aparecia como
    untracked (32 pastas dessas na máquina, uma no próprio `~`). O diário morava em
    `~/.claude/orq-retros`, que é o config dir de UMA conta, enquanto o contrato de um trabalho põe
    papéis em contas diferentes de propósito — e um executor Pi/Kimi/Codex não tem `~/.claude`.
    **Nada foi migrado**, por decisão do usuário: o que ficou no lugar antigo continua no disco,
    vira 404 no histórico do celular e some do painel de orquestração. A trava do endpoint `/file`
    não muda nada disso — ela exige que o caminho apareça no transcript, e aceita absoluto.
  - **Marcador de bloco gerenciado** (rc do shell, `~/.tmux.conf`, `keybinds.lua`, o bloco
    "Sessões-irmãs" do `~/.claude/CLAUDE.md`) virou `hangar`, e cada installer **arranca o bloco do
    marcador antigo antes** de escrever o novo — sem isso o arquivo do usuário fica com os dois, um
    deles ensinando o comando velho e que nenhum installer atualiza mais.
  - `cp-send` e companhia continuam existindo como **symlink permanente** pro mesmo script (rc
    antigo e sessão Claude já aberta chamam o nome velho); as variáveis `CP_*` e o cookie
    `cp_token` **não mudam** — quebrariam o `.env` de instalação alheia. A documentação ensina só o
    nome novo.

## Ditado: a transcrição não é o problema, o que vem depois é

(`app/transcribe.py` +
  `app/narrar.py:limpar_ditado`). Duas etapas, dois modelos: a Whisper (`whisper-large-v3-turbo`)
  ouve, e um LLM limpa. Tudo aqui foi **medido em 14/08/2026** — 5 ditados reais × 3 execuções ×
  4 modelos —, e as três coisas que mudaram valem como regra, não como preferência:
  - **O modelo da limpeza importa mais do que parece, e o critério não é tamanho — é obediência.**
    O `llama-3.3-70b-versatile`, o padrão até aqui, inventava pasta em caminho ditado
    ("backend barra app barra narrar ponto py" → `backend/barra/app/barra/narrar.py`, 3/3) e mantinha
    **as duas versões** quando a pessoa se corrigia falando. Padrão agora é `openai/gpt-oss-120b`
    (Groq, ~1,2s). O melhor dos quatro foi o `deepseek-v4-flash`, mas ele **raciocina**: 6,4s de
    mediana e **3 de 15 chamadas estourando o timeout de 8s** da limpeza — o ditado voltava cru. Com
    `reasoning_effort: "none"` ele cai pra 1,8s e acerta tudo. Daí o campo `llm_reasoning_effort`, que
    é **opcional de propósito**: vazio = a chave some do payload, porque mandá-la a um provedor que
    não a conhece é um 400 que derruba a limpeza inteira.
  - **Regra de prompt só funciona com exemplo de entrada e saída.** A regra "aplique as correções que
    a pessoa falou" era a razão de ser da limpeza e falhava 0/3 em dois modelos: eles *pontuavam* a
    correção ("A primeira é o custo do carretel. Não, desculpa. A primeira vai ser…") em vez de apagar
    a versão errada. Trocar o verbo por **APAGUE** e colar um par entrada/saída levou a 3/3. Mesmo
    padrão na regra 4 (`barra` → `/`, `traço traço` → `--`): sem o par, o `gpt-oss-120b` deixava a
    frase literal 3/3. Toda regra nova aqui **nasce com exemplo**, e com um contra-exemplo quando ela
    pode generalizar demais ("o ponto principal", "a barra de rolagem" não podem virar pontuação).
  - **Vocabulário vai pra Whisper, não pro LLM.** O `prompt` da API é enviesamento de decodificação,
    e é onde `hangar-send` para de sair "CP send". Consertar depois é impossível por construção: a
    limpeza tem ordem explícita de **preservar** nome próprio como veio, então o que a Whisper errou
    chega errado no fim. `VOCAB_BASE` (termos do app, valem pra todo mundo) + `ditado_vocabulario`
    (o que é de uma pessoa só), truncados em `_VOCAB_MAX` porque a API corta em ~224 tokens **calada**.
    `language=pt` fixo pelo mesmo motivo: sem ele, frase curta cheia de jargão inglês voltava em inglês.
  - Cuidado de cota: o prompt novo tem ~940 tokens por chamada (era ~400). No plano gratuito da Groq
    (8000 tokens/minuto) isso não incomoda um ditado por vez, mas **estoura em teste automatizado** —
    um 429 lá é cota, não qualidade; separe os dois antes de culpar o modelo.
  - **Três estilos, escolhidos na pill ao lado do microfone** (`ESTILOS_DITADO`, `ditado_estilo`,
    `components/DitadoEstiloPopover.svelte`): `limpar` (só tira hesitação e pontua), `prosa`
    (reorganiza e corta repetição — o padrão) e `briefing` (vira documento com seções). A pill fica na
    barra do composer, ao lado do microfone, e abre o MESMO popover do esforço — não um modal: é
    decisão do tamanho de escolher o esforço, e cobrir a tela pra isso é desproporcional. Existem
    porque a mesma limpeza não serve pros dois usos: ditar "abre o narrar.py" e ditar um pedido de
    dois minutos. Quem lê o estilo é o backend, então o atalho Ctrl+Espaço já grava no estilo
    escolhido sem saber que ele existe. **`briefing` é rebaixado pra `prosa` abaixo de
    `_MIN_PALAVRAS_BRIEFING`** — sem isso ele punha um `**Objetivo**` em cima de uma linha só.
  - **A trava de honestidade mudou de forma porque a antiga proibia o que o usuário pediu.** Contar
    palavra nova crua (o guarda antigo) rejeitava qualquer estruturação: `Objetivo:`, `-`, e até
    escrever "tô" como "estou" contavam como conteúdo inventado — 8 "palavras novas" num ditado
    real, com 100% do conteúdo preservado. Agora são duas medidas, e as duas foram calibradas
    contra os mesmos casos: **cobertura** (quanto do conteúdo da pessoa sobreviveu; pega o modelo
    que resumiu ou respondeu) e **`_conteudo_novo`** (palavra de conteúdo que ela não falou). Medido: defeito 4 palavras novas, limpeza honesta 0, prosa real 1 → teto 2. **Cobertura
    sozinha não separa** (defeito 79%, prosa legítima 75%), e é por isso que as duas coexistem.
  - **O `briefing` NÃO paga a trava de invenção** (`_Travas.cobra_invencao`), e isso é decisão do
    usuário, não descuido: "no briefing minhas palavras vão mudar; se eu estiver em prosa, aí
    beleza, não mudar minhas palavras". `limpar` e `prosa` não reescrevem — um pontua, o outro
    reordena —, então ali palavra nova é palavra que a pessoa não disse. O briefing reescreve por
    definição, e cobrar dele é recusar o serviço pedido: medido, um briefing bom com 98% de
    cobertura foi rejeitado por 4 "invenções" que eram conjugação. Ele segue protegido pelo teto de
    tamanho, pelo piso de cobertura e pela recusa de saída vazia.
  - **Comparação é por RADICAL** (`_radical`), não pela palavra inteira. `clicava`/`clico`/`clicar`
    caem no mesmo balde. Sem isso a trava punia conjugação — a mesma classe de erro que
    `_CONTRACOES` resolveu pra `tô`/`estou` e que voltou por outra porta. **Mas a vogal final só cai
    com prova de verbo no próprio texto** (`_raizes_de_verbo`, alimentado pelos DOIS textos): cortá-la
    sempre juntava `posto`/`posta` e `conta`/`conto` no mesmo radical — o par que o comentário do piso
    usava como exemplo do que não podia acontecer —, e aí trocar "a conta do cliente" por "o conto do
    cliente" passava com 0 palavra nova e 100% de cobertura, calado, justo em `limpar` e `prosa`.
    Sufixo de verbo (`ava`, `ando`, `ar`, …) e derivação (`mente`, `dade`, …) cortam sempre; plural
    (`s`) também. Contra-exemplo travado em `test_troca_de_genero_ainda_e_palavra_nova`.
  - `_CONTRACOES` iguala fala reduzida à forma escrita (`tô`→`estou`, `pra`→`para`) **antes** de
    qualquer comparação. Sem isso a limpeza melhora o texto e é punida por isso.
  - **Raciocínio piora e não é questão de calibragem.** Testado com os dois ditados reais: com
    `reasoning_effort` ligado, 4 de 9 execuções estouraram 25s e a única prosa que voltou levou
    14,9s, contra 2,3–3,2s desligado. Num teste anterior o modelo pensando ainda comeu o "não o
    redis" de "usa o postgres não o redis", lendo negação como autocorreção. Pensar sobre um texto
    vira interpretar o texto, e aqui interpretar é o defeito.
  - **Quem manda no estilo é a PILL, não a config** (`?estilo=` no `/transcribe` →
    `narrar._estilo_efetivo(cru, pedido)`). O estilo mora no servidor, mas o app o lê **uma vez por
    carga de página** (`lib/ditadoEstilo.svelte.ts`): uma troca feita noutra aba ou noutro aparelho
    nunca chegava na tela aberta, e em 21/08/2026 a pill dizia "Só limpar" enquanto o servidor
    guardava `briefing` — o ditado voltou estruturado sem ninguém ter pedido. O front manda junto o
    rótulo que a pessoa **leu antes de falar**, e ele vence; ausente ou desconhecido, a config
    decide como sempre. Duas amarras: o front só manda quando o store já leu o servidor
    (`ditadoEstilo.pronto` — mandar o padrão chutado seria o app sobrescrevendo a escolha dela com
    palpite), e o popover **revalida** ao abrir, pra a lista parar de exibir valor de horas atrás.
  - **O teto de tempo é rede contra pendurar, e ele mora nas DUAS pontas.** O do navegador era
    120s (`lib/api.ts`) enquanto o backend podia gastar 120s de Whisper **mais** a limpeza: a
    requisição era abortada com o trabalho em curso e a pessoa perdia o ditado inteiro por causa do
    relógio do cliente. Hoje: 300s no cliente, e 60/90/120s por estilo no servidor (subidos em
    21/08/2026 — o `muse-spark-1.2-contributor-free` do OpenCode Zen levou 16,4s pra limpar UMA
    frase, contra ~1,2s do `gpt-oss-120b` na Groq). Quem estoura ainda **não perde o áudio**: o
    `Composer` guarda o `File` da tentativa que falhou e oferece "Transcrever de novo" ao lado do
    erro — o áudio já existe, mandar a pessoa repetir dois minutos de fala é que era o defeito.
  - **O provedor da limpeza é trocável pela tela, e não só a Groq** (Configurações → Servidor →
    Avançado: Endpoint / Chave / Modelo / Raciocínio do LLM → `llm_*` em
    `~/.claude/runtime-config.json`). `_provedor()` só lê `llm_api_key` quando há `llm_base_url`
    próprio; endpoint vazio reutiliza a `groq_api_key` somente quando a transcrição também usa o
    serviço padrão. E o **briefing tem provedor próprio**
    (`llm_briefing_*`, `_provedor("briefing")`), porque os dois usos não pedem o mesmo modelo:
    limpar e prosa querem rapidez — a pessoa está olhando o campo esperando o texto —, o briefing
    quer quem estrutura melhor e pode demorar. Medido aqui: Groq/`gpt-oss-120b` 2,1s no limpar,
    OpenCode Zen/`muse-spark-1.2-contributor-free` 9,0s no briefing. O perfil sai do **estilo**,
    nunca de um flag à parte, e endpoint de briefing vazio cai no provedor de sempre. Provedor
    lento muda o que as travas veem:
    medido no muse-spark, um ditado com autocorreção longa cai pra 62% de cobertura e o estilo
    `limpar` (piso 0,80) devolve o cru com aviso — o modelo apagou a versão corrigida, que é a
    regra funcionando, mas o piso de `limpar` não foi calibrado nele.
  - **Provedor que falha cai na assinatura Claude da própria máquina** (`narrar._via_claude`), e o
    caminho é o `claude -p`, não o SDK — o `claude-agent-sdk` roda esse mesmo binário por baixo,
    seria dependência nova pra chegar no mesmo `subprocess`. O gatilho é falha do PROVEDOR (HTTP,
    rede, JSON inválido, payload sem texto); **sem chave configurada continua 503**, porque config
    ausente é pra corrigir na tela, não pra mascarar gastando cota. Falhando os dois, quem sobe é o
    erro ORIGINAL do provedor — "o plano B também falhou" é ruído em cima do que a pessoa conserta.
    O que motivou: em 08/09/2026 o OpenCode Zen respondeu **500 nos dois** modelos que uma chave
    `contributor` alcança (`muse-spark-1.3-contributor` e `1.2-contributor`; o `1.2-…-free` virou
    401 "not supported", e glm/deepseek dão 400 nessa chave), e cada ditado voltava cru.
    Três números medidos no mesmo dia, com o system prompt real do estilo `limpar`:
    **sonnet 3,6–4,5s contra haiku 27,8–39,2s** (consistente em 3 rodadas, não é primeira chamada —
    daí o sonnet fixo); **`--tools ""` leva a entrada de ~18.900 tokens pra 466**, porque o caro são
    as definições das ferramentas, não o prompt; e o par `--setting-sources ""` + `--strict-mcp-config`
    evita carregar settings/hooks/skills e subir servidor MCP pra limpar uma frase. O prompt vai por
    **stdin**: ditado de dois minutos passa do limite de argv e apareceria inteiro no `ps`.
    Duas coisas que o fallback NÃO muda, medidas: o texto dele passa pelas mesmas travas (um resumo
    volta como cru, igual ao do provedor), e a `_cobertura` continua rejeitando o ditado curto cheio
    de `barra`/`traço traço` — 0,727 contra o piso 0,80 de `limpar`, idêntico pela Groq.

## Transcrição, organização do texto e leitura são capacidades separadas

(`VozSettings.svelte` + `transcribe.py` + `narrar._provedor`, 17/09/2026.) A tela móvel mostrava
`Voz` duas vezes, depois `Ditar`, `Transcrever`, duas chaves de LLM sem relação explícita e ajustes
de voz que só funcionam com ElevenLabs. A captura real também mostrou o efeito mais perigoso do
vocabulário: uma chave de outro serviço de transcrição parecia poder alimentar a organização do
texto, mas o backend a enviaria para o endpoint padrão do LLM.

- A transcrição tem endpoint e modelo próprios, compatíveis com a API da OpenAI; vazios preservam
  o serviço e o modelo anteriores. O nome do provedor padrão aparece só no guia de criação de chave,
  não no rótulo da capacidade.
- Endpoint próprio de transcrição torna a chave exclusiva do áudio. Sem `llm_base_url` e
  `llm_api_key` próprios, a organização fica indisponível; a chave nunca viaja para o host padrão.
- ElevenLabs e comando local são alternativas de leitura. Voz, amostra e naturalidade só montam
  dentro da opção ElevenLabs; comando local não finge oferecer controles que não entende.
- `null` no `POST /api/config` remove o override e volta ao valor do ambiente. A tela oferece essa
  ação apenas quando `origem == app`; valor vindo do `.env` continua visível, mas não apagável pelo
  navegador.

## Grupo: o protocolo é do HOOK, a saída é de UMA esteira, e o anti-loop é do backend


  (`app/pair_texto.py` + `hooks/pair_hook.py` + `api._avisar_saida` + `registry._varrer_pares_mortos`,
  02/09/2026). Decisões que fecham furos medidos na análise daquele dia:
  - **Protocolo reinjetado no `SessionStart`** (`startup|resume|clear|compact`) a partir do sidecar
    `.hangar-pair/<nome>.json`. O prompt do `--pair` sumia no `/clear` e na compactação; o badge
    ficava e o modelo esquecia os pares. O `pair_dir` vai por argv porque é o do BACKEND — sessão em
    `--conta` tem `CLAUDE_CONFIG_DIR` próprio, e o sidecar não mora lá. Por isso os textos moram em
    `pair_texto.py`, stdlib-only (mesma regra do `engines.py`).
  - **Protocolo completo só pro recém-chegado** (`snap[m] is None`); veterano recebe "fulano entrou";
    peers e tarefa iguais = nada. Adicionar o 5º membro disparava 5 prompts de 1,5KB, 4 redundantes.
  - **Tarefa diferente da existente é 409** sem `--substituir-tarefa` — cada `--pair` de um árbitro
    sobrescrevia a de todos, calado.
  - **Toda saída avisa quem ficou pela mesma esteira**: unpair, kill (não avisava ninguém — os pares
    mandavam recado pra nome morto ou pra sessão nova que o reusasse) e morte fora do app. Remoto vai
    por `/unpair-remote` no unpair e no kill; na varredura só loga (rede dentro do `list()` não).
  - **Varredura de morto fora do app roda no fim de `list()`, e três coisas a seguram:** contador
    DE CLASSE (há 4 instâncias de `SessionRegistry` — api, sse×2, prune — e todas chamam `list()`);
    ausência confirmada por **tempo** (`_PAIR_AUSENCIA_MIN_S`), não por número de polls, porque
    `kill()` e `rename()` chamam `list()` numa janela em que o nome está ausente de propósito; e lista
    vazia = tmux fora = não varre, senão dissolvia todo grupo da máquina. Aviso pela fila durável
    (nunca send-keys ali) + drain por callback (`apos_saida_por_morte`), porque a fila só drena em
    transição de hook e o peer já ocioso nunca receberia. O dict de classe (`_pair_ausencias`) é
    limpo com `pop(n, None)`, nunca `del` — as 4 instâncias varrem concorrentemente e outra thread
    pode já ter tirado a mesma chave.
  - **`--group` recusa `[grupo:`/`[de:` reencaminhado e limita 5/min por gid** (429). Todo membro
    recebe pelo backend (escada abaixo); `pulados` fica vazio e é mantido só por compatibilidade.
  - **Recado de sessão-irmã: o backend escreve no socket nativo do Claude Code; o modelo nunca
    escolhe transporte.** Escada em `_send_one`/`_send_one_headless`: socket nativo → plugin sem
    tecla → tmux → fila; `hangar-send` só imprime "entregue"/"na fila"/erro. Até 21/09/2026 o
    script RECUSAVA (código 3) quando os dois lados tinham socket e mandava o modelo usar
    `SendMessage`; a `tardis-control` leu a recusa como entrega e um kick-off pra sessão
    recém-nascida se perdeu. A frase "o socket do Claude Code está fora do alcance do backend"
    era suposição: medido em 21/09/2026 com um socket falso capturando o `SendMessage`, o quadro é
    uma linha JSON (`{"msgV":1,"msg_id","type":"user","message":{"role","content":"<cross-session-message
    from=… from-name=… from-mode=…>…</cross-session-message>"},"priority":"next","from":"uds:…"}`),
    sem autenticação no Linux (token só no Windows), e um cliente Python entregou numa sessão viva
    (`app/uds_messaging.py`). O pid/socket vem do registro do próprio CLI
    (`<config>/sessions/<pid>.json`, campo `messagingSocketPath`), que cobre a sessão sem terminal —
    `inbox_socket_of` pelo pane devolvia `null` nela. `from` é endereço de RESPOSTA: o backend liga
    `cc-socks/<pid>.sock` próprio pra receber `peer_message_status` (retido/recusado) e avisa a
    remetente com `[painel: entrega de recado]`. O recado vai com o prefixo `[de: X]` no corpo e o
    parser não o dobra (`_PEER_PREFIXO_RE`), pra `[grupo:]` sobreviver ao envelope.
  - **Aviso do app sai como `[painel: <rótulo com espaço>]`, nunca `[de: …]`** (`pair_texto.PREFIXO`,
    24/09/2026). Os avisos de grupo saíam `[de: hangar]` e terminavam em "Confirme em uma linha": o
    uma sessão solta num grupo pelo arrasto confirmou com `hangar-send hangar …` e o
    recado caiu na sessão `hangar` (nome padrão de quem abre o repo), que não era do grupo. O socket
    nativo também tira o remetente do prefixo (`separar_prefixo`), então o envelope dizia
    `from-name="hangar"`. Nome de sessão só tem `[A-Za-z0-9_-]` (`names.py`): rótulo com espaço
    nunca coincide com um. Teste real com três sessões Haiku: todas confirmaram no próprio terminal.
  - **Soltar uma sessão avulsa num grupo existente só entra**: o diálogo mostra a tarefa do grupo,
    sem campo nem "Sugerir", e manda tarefa vazia (o `join_group` herda). Campo e sugestão ficam
    para grupo novo ou fusão de dois grupos.
  - Contrato do grupo dissolvido vai pra `~/.hangar/pair-arquivo/`, não pro `unlink`.
  - **Teste que chega em `SessionRegistry.list()` ou num `pair.leave()` de último membro isola
    `pair.settings.projects_dir`, zera `SessionRegistry._pair_ausencias`, anula
    `registry.apos_saida_por_morte` e faz patch de `pair._arquivo_dir`** — sem as quatro, 2 vezes
    neste plano uma suíte verde mexeu de verdade no `~/.claude/.hangar-pair`/`~/.hangar/pair-arquivo`
    do desenvolvedor.

## Plan progress

(`app/planprog.py` + `registry._decorate_plan` + `PlanBar`/`PlanPanel.svelte`):
  the source of truth is the plan's own `.md` under `docs/superpowers/plans/` — no separate state
  file, `parse_plan` re-reads it and re-counts `- [x] **Step …**` on every discovery. Fenced blocks
  (` ``` `/`~~~`) are stripped **before** the regex runs, **preserving byte offsets** (chars → spaces,
  `\n` kept) — plans show example steps inside code fences, and without the strip a freshly-written
  plan is born "3/47 done" (measured on this very plan: 53 matched vs 48 real). The decoration runs
  **inside** the git `to_thread` (`registry.py:851`), never in the coroutine — same precedent as the
  2026-07-23 incident. `_list_sig` (`sse.py`) carries `plan_name` alongside `plan_done`/`plan_total`:
  switching from plan A to plan B that happens to also read `9/17` wouldn't re-emit the list and the
  chip would stick on the wrong plan — the same bug class as `engine`. `plan_tasks` (one `(done,total)`
  per Task) rides the payload too, because the segmented bar can't be derived from
  `task_idx`/`task_total` alone — it would lie whenever an earlier Task still had a pending step.
  `_plans_dir` climbs up to 6 levels looking for `docs/superpowers/plans/` but **stops at the first
  `.git`** — without that, a worktree with no plans of its own would climb into the main checkout and
  show someone else's plan ("no bar" is a limitation, "wrong bar" is a bug). Executing a superpowers
  plan: mark `- [ ]` → `- [x]` at the end of each Step — that's what feeds this feature.

## Cota: cache em disco e espera após 429

(`app/cotas.py`, 14/09/2026.) O cache das cotas era só memória: cada restart do backend relia
todas as credenciais de uma vez. Três restarts em 3 min (14:04–14:07) fizeram a API de uso da
Anthropic responder 429 pras 5 contas Claude, e a aba Contas mostrou "não informa cota" com tudo
conectado. Duas regras: o cache vai pra `~/.hangar/cotas-cache.json` (pasta do Hangar, não da
conta) e o que está dentro do TTL volta do disco na subida; e fonte que levou 429 só vence de novo
depois de `_ESPERA_429_S` (10 min) — insistir no próximo poll só renova o 429. Sob pytest o disco
fica fora: a suíte gravaria fontes de mentira no arquivo real da máquina.

## Redefinições guardadas do Codex respeitam a janela semanal

(`app/cotas.py` + `codex_contas_api.py` + `ContasSettings.svelte`, 17/09/2026,
codex-cli 0.154.0.) `account/rateLimits/read` devolve `rateLimitResetCredits` junto das janelas, e
`account/rateLimitResetCredit/consume` gasta uma redefinição com chave idempotente. Na leitura real,
a conta padrão tinha zero e uma conta adicional tinha duas; ambas ainda possuíam cota semanal.

- A tela mostra quantidade, expiração e o reset de cada janela. Janela longa leva dia da semana,
  data numérica e hora (`dom 04/10 2h`), porque só o nome do dia fica ambíguo. O consumo só é
  habilitado quando a janela de 7 dias está em 100%.
- O backend relê `account/rateLimits/read` imediatamente antes do consumo e recusa se a janela
  semanal ainda tiver saldo. O botão não é barreira de segurança.
- Uma tentativa usa UUID e conserva a mesma chave ao repetir depois de falha de transporte.
  `alreadyRedeemed` é confirmação de uma tentativa anterior, não um segundo consumo.
- `nothingToReset` e `noCredit` são resultados sem sucesso. Todo resultado definitivo força nova
  leitura da credencial; falha nessa releitura preserva a informação de que o consumo já ocorreu.

## Registro de peer nunca grava loopback, e o endereço torto tem frase própria

(`frontend/src/lib/registrarPeerDoisLados.ts`, `lib/maquinas.ts`,
`components/settings/{ListaMaquinas,DetalheServidor,MaquinasSettings}.svelte`, 21/09/2026.)
Ligar "Recados entre sessões" entre duas máquinas desta malha deixava o par pela metade, e a tela
só dizia "Recados só de ida". Medido nas duas pontas: a viana guardava
`casa -> http://127.0.0.1:8765`, e `GET /api/peers/check` NELA devolvia
`{"estado":"estranho","identificador":"viana"}` — ela batia em si mesma. Com o endereço do
Tailscale a mesma chamada devolvia `{"estado":"ok","identificador":"casa","tempo_ms":35}`.

A causa é que o registro gravava no peer a URL que o NAVEGADOR usa para o dono (`meuBase =
dono.baseUrl`). No desktop essa URL é `http://127.0.0.1:8765`, que do outro lado do fio é o
próprio peer. **O padrão continua sendo a URL do navegador — ela é a única medida de verdade que
o aparelho tem —, mas loopback nunca: aí quem responde é `/api/alcance` do dono, que já mediu por
onde chegam nele.** Não é heurística de rede: loopback gravado num peer é errado por construção,
e falha PARECENDO registrado.

Três frases separadas onde havia uma. `estranho` ganhou tipo próprio (`ida_outra_maquina` /
`volta_outra_maquina`) antes do `parcial`, que é o balde genérico e engolia o único modo de falha
que se conserta trocando um endereço em vez de esperar a máquina voltar — e o bloco de correção
agora abre nele. E "não responde ou não tem identificador" virou três: o 401 é o token deste
aparelho recusado, a falha de rede é a máquina fora do ar, e o identificador vazio é um campo
para preencher — que passou a existir no detalhe de QUALQUER servidor com token aqui, gravando o
`CP_SERVER_ID` no `.env` dele. Antes, o aviso não tinha campo nenhum: era preciso trocar o
servidor da tela inteira para preencher o nome de outra máquina.

Um terceiro erro apareceu junto: o bloco de correção pergunta "qual endereço o X deve usar para
chegar aqui?" e gravava a resposta como `base_url` do PRÓPRIO X — consertava o lado oposto ao que
a frase promete. O endereço digitado é o do dono, e vai no peer.

## Servidor que não responde esfria; o interruptor manual não bastava

(`packages/core/src/esfriamento.ts`, `api.ts`, `frontend/src/lib/sessionsStore.svelte.ts`,
16/09/2026.) Máquina desligada era procurada para sempre. Medido com dois PCs Windows fora do ar
havia um dia: **87 tentativas em 15 minutos, mediana de 10 s entre elas**, cada uma pendurando até
o prazo inteiro — VPN para nó morto não recusa conexão, ela engole. Quem abria os sockets era o
app (Electron, pelo `ss -tnp state syn-sent`), por fora do backoff de 60 s que o stream de lista já
tinha. O único jeito de calar era `"enabled": false` no `peers.json`, que é interruptor MANUAL: a
máquina sumia do painel e só voltava quando alguém editava o arquivo.

**A primeira regra escrita não bastou, e o porquê importa.** Ela era uma escala de espera: três
falhas, 1/2/5/15 min, retomada automática. Medido no iPhone depois de publicada: as tentativas
caíram de 24/min para 7–15/min e **nunca chegaram a zero**, com silêncios de 101 s no meio (a
espera de 1 min funcionando). A causa é o iOS, que descarrega e recarrega o PWA em segundo plano o
tempo todo: cada retomada zerava o contador em memória e o aparelho recomeçava as três tentativas
por servidor.

A regra adotada naquele momento foi: **uma falha de REDE já marca o servidor como desligado, e ele só
volta a ser procurado quando a pessoa mandar** — não há retomada por tempo. O estado vai para o
`localStorage`, senão o recarregamento do app apaga o que já foi aprendido. Erro HTTP não conta: a
máquina respondeu, e marcá-la esconderia o erro que precisa aparecer. Abrir a lista dos offline na
barra lateral é o "buscar agora" e libera todos. `enabled: false` no `peers.json` continua
existindo para a máquina que se quer fora de propósito.

Uma resposta recebida em outra tela também comprova que a máquina voltou. Em 22/09/2026, o
Delphi-02 respondia à tela Máquinas, mas sua lista continuava na última sessão conhecida: os
clientes de peers/alcance não retiravam a marca de falha, e o stream encerrado não era reaberto.
Essas respostas agora liberam e reconectam somente o servidor que respondeu. Falha de rede
mantém a marca. Expandir o resumo dos offline não libera novas tentativas; Reconectar antecipa
a tentativa por servidor ou para todos, conforme o botão usado. Marca sem lista carregada aparece no
resumo offline, em vez de sumir.

Ainda em 22/09/2026, o usuário substituiu o bloqueio permanente por recuperação automática.
Agora cada falha agenda outra tentativa em 30 s, 1 min, 2 min, 4 min, 5 min, 10 min e,
daí em diante, 30 min. Uma resposta limpa a contagem; a próxima queda recomeça em 30 s.
Prazo absoluto e contagem ficam no armazenamento: recarregar não reinicia a espera nem libera
consultas antecipadas. A marca antiga sem prazo migra uma vez para 30 s. Expirar só permite
tentar; a marca offline sai quando há resposta. A regra também vale para o servidor ativo.
Sessões de uma rota offline ficam fora da agregação visual, preservando o cache interno; assim
uma rota indisponível não esconde uma cópia saudável da mesma sessão pela deduplicação.

Na mesma data, o Ctrl+R reproduziu uma falsa queda: o `EventSource.onerror` gravou a marca 6 ms
depois do `beforeunload`, antes do `pagehide`. A saída da página agora fecha os streams e cancela
seus prazos, protegendo também os fetches cancelados pela navegação. Eventos de uma conexão
substituída não alteram a atual. `pageshow` restaura os streams ao voltar pelo histórico, sem
apagar marcas de falhas reais.

Em 23/09/2026 o usuário pediu que máquina que já respondeu não passe pela mesma regra. Diário do
iPhone: o app abriu às 06:19:29 e foi para o segundo plano 1 s depois, antes de as listas
chegarem; na volta, às 06:19:45, o iOS entregou o erro do socket morto na suspensão 18 ms ANTES do
`visibilitychange`, e a marca de 30 s gravou. A liberação daquele dia só olhava quem tinha lista
ao esconder, então não soltou ninguém; fechar e reabrir o app herdava o prazo (restavam 19 s,
depois 14 s) até conectar em 89 ms às 06:20:23. Agora a última resposta de cada servidor fica
gravada (`hangar_servidores_responderam`, uma escrita por minuto no máximo). Quem respondeu nas
últimas 24 h não escala, fica sempre em 30 s, e é liberado na hora quando o app abre com a tela
à vista ou volta a ficar visível. Recarga em segundo plano continua respeitando o prazo, e a
máquina sem resposta há mais de 24 h volta à escala inteira.

Na mesma conversa o usuário apontou o resto do defeito: com o app em segundo plano a queda ainda
contava, marcava offline e cada nova tentativa escondida subia a espera. Falha com o app escondido
agora não chama `registrarFalha` (o `definirProtegido` cobre também os `fetch`), e a nova tentativa
escondida sai a cada 30 s fixos, para não martelar máquina desligada com o desktop minimizado.
O estado de segundo plano vem do evento `visibilitychange`, não do `document.visibilityState`:
o erro da suspensão chega antes do evento da volta, e só o evento garante que ele ainda cai como
segundo plano.

Ainda em 23/09/2026 a primeira espera de 30 s virou o problema: com o backend local reiniciando
(ou uma queda de um instante com a máquina no ar), a lista marcava a máquina em que a pessoa
estava como offline e só voltava 30 s depois, ou com Ctrl+Shift+R. Recarregar com o backend ainda
subindo gravava a mesma marca de novo. O usuário pediu que a primeira falha não espere. A escala
agora começa em 2 s e 5 s, e só a terceira falha seguida chega aos 30 s; quem respondeu nas
últimas 24 h sobe 2 s → 5 s → 30 s e para aí. Máquina desligada de verdade paga duas tentativas a
mais antes de esfriar, o que não pesa na VPN do iPhone (o problema lá eram 13–24 por minuto).

Só a escala não resolveu o restart: medido com `systemctl --user restart`, o backend levou 10 s
para parar e mais 4 s para subir, as três tentativas (2 s, 5 s) caíram dentro desses 14 s e a
máquina ficou offline 23 s DEPOIS de voltar. Por isso volta a proteção que existia antes de
22/09 (e que aquela data tinha tirado do servidor ativo): o servidor ativo e o dono da URL da
página nunca recebem a marca, e tentam de novo em 1, 2, 4, 8, 16 s até o teto de 30 s, em memória.
Marca antiga gravada para eles é apagada ao conectar.

**O custo real não era bateria, era a VPN do iPhone.** Com o cabo USB e o `idevicesyslog`, o log de
dentro do aparelho mostrou a mesma varredura acontecendo na extensão de rede do Tailscale
(`IPNExtension`), a 13–24 tentativas por minuto, cada uma um `open-conn-track: timeout opening ...
online=no`. O Tailscale iOS carimba o uso de memória nas linhas dele: a extensão ia de 26,6 MB para
31,9 MB num processo e de 35,1 MB para 44,9 MB no seguinte, contra o teto de 50 MB que o iOS dá a
uma Network Extension. Nas quedas a extensão **não morria** (seguia escrevendo no log), mas parava
de responder pelo caminho direto e pelo DERP ao mesmo tempo — que é o aviso `MagicSock Function
ReceiveDERP is not running` na tela, e o motivo de só religar a VPN resolver: processo novo,
memória zerada.

Duas hipóteses descartadas com medição pelo caminho, para não voltarem: não é o `AskUserQuestion`
(em toda a janela de queda, `app_pergunta_aberta` = 0, e o push nem está configurado nesta
máquina), e não é o IPv6 — a correlação era forte (1495 amostras boas em IPv4 contra 72 falhas em
74 amostras IPv6), mas com o IPv6 desligado na interface a queda voltou a acontecer em IPv4.

As sondas que produziram isso ficam em `~/.hangar/diag/` (da máquina, fora do repositório):
`sonda-iphone.py` amostra rede + estado do app a cada 2 s, e um coletor do `idevicesyslog` guarda o
lado de dentro do iPhone.

## Compartilhado por sessão, não por conexão

(`app/difusor.py`, `stats.Accumulator.
  compartilhado`, `registry._atualizar_git`, 04/09/2026). Três coisas que cada SSE refazia
  sozinho: o monitor de estado (desktop + celular = 2× `has-session` + `capture-pane` a cada
  0,75s), o acumulador de estatísticas (relia o transcript inteiro por conexão — 109–204ms num
  de 21 MiB, contra 0,08ms no acumulador já quente) e o git da listagem (`git status` + `diff`
  em série por sessão ANTES de publicar o estado; um repositório lento segurava o card de todas).
  O `Difusor` é genérico: uma fonte por chave, cada ouvinte recebe o último evento ao entrar
  (o monitor só emite em mudança) e cópia dos seguintes; a fonte morre com o último ouvinte. A
  chave do monitor leva o transcript, e o `__reset__` do `/clear` recria o `state_task`: o
  monitor fecha sobre o sid da conexão que o criou, e sem isso quem ficasse herdaria a closure
  de uma conexão já morta. O git agora sai da lista com o último número bom por cwd e atualiza
  em segundo plano (single-flight por cwd; resultado `None` não apaga o anterior — erro nunca
  vira "repositório limpo"). Custo: o primeiro poll depois de subir o backend sai sem badge de git.
  **Medido e NÃO mexido: o re-render da prévia a cada 33ms.** No app desktop (Electron, esta
  máquina), uma resposta de 9,8k chars em streaming = 797 atualizações da prévia, 2 long tasks
  (51 e 61ms), quadro p95 de 33ms, 5 quadros acima de 50ms em 38s. Não justifica trocar o
  parse inteiro por parse do último bloco (Markdown novo muda a leitura do anterior). No celular
  não foi medido — a sonda é `PerformanceObserver('longtask')` + `requestAnimationFrame` na
  página do chat, e teria que rodar no iPhone.

## MCP do Hangar: identidade no cabeçalho, o backend resolve; token nunca no pane

(`app/mcp_server.py`, `app/quem_chama.py`, `scripts/registrar-mcp.py`, 16/09/2026). As tools
`quem_sou`/`sessoes`/`enviar` são o `hangar-send` como tool tipada, servidas pelo backend em
`/mcp` com o SDK oficial `mcp` 2.2.0 (provado em Python 3.14.6 antes de decidir: a primeira
versão da spec propunha JSON-RPC à mão por duas dúvidas não verificadas, e a prova de cinco
minutos derrubou as duas). `app.mount()` passa por fora do `Depends(require_auth)`, então o
bearer é conferido num embrulho ASGI antes do sub-app; o gerenciador de sessões do SDK só roda
uma vez por instância, então o sub-app nasce no lifespan, não no import. A proteção contra DNS
rebinding do SDK fica ligada com `127.0.0.1`/`localhost` com e sem porta (o padrão só aceita
com porta e o teste em ASGI não manda porta). Erro esperado é `ToolError`, senão o modelo vê só
`Error executing tool`.
**Identidade**: quem chama manda `X-Hangar-Key` (sessão sem terminal), `X-Hangar-Pane` e
`X-Hangar-Session`; chave vence pane (o filho sem terminal pode herdar `TMUX_PANE` do pai),
pane vence nome (rename não reescreve o env do processo), pane em mais de uma sessão (psmux)
não resolve, e nada resolvido é erro — nunca `cli`. Mesma regra do `me()` do CLI, que continua
resolvendo localmente porque funciona com o backend caído. **Token**: Claude Code recebe pelo
`headersHelper` (script que lê o `backend/.env` a cada conexão); Codex por `http_headers` no
`config.toml` (0600). Nunca exportado no pane: `printenv` num turno mandaria o token pro
transcript. Prova ponta a ponta: `enviar` de `hangar-2` pra `cx-pergunta` entregou
`[de: hangar-2] …` e a resposta `ok` voltou pelo caminho de sempre.

## Arquivo citado na conversa é editável, com a citação como consentimento

17/09/2026. Um arquivo fora da raiz da sessão abria no visor em modo leitura: `abrirExterno`
gravava `digest: null` e o `FileViewer` só liga o botão de salvar quando há digest. Quem
tropeçou nisso foi o caminho mais comum — o agente cita `~/.claude/settings.json` numa resposta,
a pessoa clica, a tela abre, e não dá pra mudar nada ali.

A trava que faltava para a escrita já existia para a leitura: `serve_file` só serve um caminho
que **aparece no transcript desta sessão**. Citado por quem usa ou pelo agente = consentido.
Ela virou `_resolver_citado()` e passou a valer para os dois lados, com `GET`/`POST
/api/sessions/{name}/file/text` ao lado do `/files/read` e `/files/write` da árvore: mesma
mecânica do `filetree` (teto de 512 KB, recusa de binário, digest da leitura, tmp+rename
preservando o modo), outra política de caminho — a raiz da sessão lá, a citação aqui. O
`filetree` ganhou `read_at`/`write_at`, que é a mecânica sem `_resolver`; `read_file` e
`write_file` continuam sendo `_resolver` + a mesma chamada.

**Por que a citação basta como autorização.** Quem tem o bearer do Hangar já pode mandar um
prompt e fazer o agente editar qualquer arquivo do disco. A escrita direta não amplia o alcance
de um invasor, só encurta o caminho. O risco que sobra é engano de quem usa, e contra ele valem
o digest (recusa se o arquivo mudou no disco desde a leitura) e o fato de o arquivo já estar
aberto na tela. A pasta `.git` fica de fora aqui também, pela mesma regra do `_protege_git`:
componente `.git` sobre o **realpath**, então `atalho -> .git` não escapa.

Medido ao vivo, backend reiniciado e front rebuildado: `GET /file/text` devolveu o digest de um
`/tmp` citado; `POST` com o digest certo gravou; com o digest velho voltou 409
`erro_arq_mudou_no_disco`; caminho nunca citado voltou 403 `erro_arquivo_nao_citado` nos dois
verbos; `.git/config` citado voltou 403 `erro_arq_area_do_git`. Na tela, pelo navegador
embutido: o arquivo de fora da raiz abriu com o botão **Editar**, e o Salvar mudou o conteúdo no
disco. Um primeiro teste com `/etc/hosts` deu 200 e parecia furo — era o transcript, que já o
citava 5 vezes; a trava estava certa e o teste, errado.

## Chave do Jev: no runtime_config, e na sessão só por escolha da abertura

18/09/2026. O Jev (typesafe.ai) decide a navegação do `hangar-preview objetivo`, e a chave estava
em claro no `~/.claude/settings.json` — que é symlink compartilhado por TODAS as contas. Ela passou
para `runtime_config.EDITAVEIS` (`jev_api_key`, e o LLM pequeno opcional em `jev_texto_*`), no mesmo
lugar e com o mesmo tratamento da chave da Groq: editável sem reiniciar o serviço, e listada em
`SEGREDOS`, então volta mascarada e nunca inteira. O cadastro em `SEGREDOS` é EXPLÍCITO mesmo com a
rede `_PALAVRAS_DE_SEGREDO` já cobrindo `_key`: depender do acaso do nome quebra calado no dia em
que alguém renomeia o campo.

**O que isso NÃO é:** sigilo. Numa sessão aberta com o recurso ligado a chave está no ambiente do
processo e é legível por quem já roda lá dentro. O ganho é ser por sessão e por escolha, em vez de
global e em claro num arquivo que todas as contas compartilham — e é assim que vale escrever, sem
arredondar para "agora está seguro".

**Ligar é escolha da ABERTURA**, pelo mecanismo que já existia para o `CLAUDE_CODE_SUBAGENT_MODEL`:
`POST /api/sessions` ganhou `jev`, que vira `-e` no `tmux new-session` e chave do `env` do filho nos
dois caminhos sem terminal. O marcador `HANGAR_JEV=on|off` vai SEMPRE — sem ele o verbo não separa
"desligado nesta sessão" de "nunca configurado", e as duas pedem frases diferentes. Desligado é o
padrão, e o relançamento (troca de modelo, resume) relê o marcador do `/proc` do processo que vai
morrer, mas relê a CHAVE do runtime_config: sessão ressuscitada não fica presa numa chave trocada.

Não dá para virar no meio da sessão, e foi a escolha: o pedido era rodar a mesma tarefa com e sem o
Jev e comparar, o que acontece entre sessões. Virar ao vivo exigiria endpoint novo, estado por
sessão e a chave atravessando mais uma fronteira. Ficaram de fora, pelo mesmo motivo, o registro de
consumidores da chave e o override por sessão. A tela é só do front desktop neste primeiro momento,
por decisão explícita — o app Expo fica para depois.

## Function hooks: configuração do servidor, e por isso o relançamento relê

18/09/2026. `CLAUDE_CODE_ENABLE_FUNCTION_HOOKS=1` é o portão de acesso antecipado sem o qual o
Claude Code nem lê plugin de function hook. Estava no `~/.claude/settings.json`, onde vale para toda
conta e toda sessão — inclusive as que não têm nada a ver. Virou `claude_function_hooks` em
`runtime_config`, com interruptor no card do Claude na tela de Harnesses, ao lado do de atualizar a
statusline, que é o precedente exato: booleano do runtime_config desenhado como `switch`, com a
ajuda em `aria-describedby` e não dentro do nome.

**Não é escolha por sessão, é configuração do servidor** — e isso muda duas coisas em relação ao
`jev`, que mora ao lado e parece igual:

- **Não há marcador `on|off`.** O `HANGAR_JEV` existe porque o `hangar-preview objetivo` precisa
  separar "desligado nesta sessão" de "nunca configurado" em frases diferentes. Aqui não existe
  "nunca configurado": a ausência da variável É o desligado, e é ela que o Claude Code lê. Copiar o
  marcador por simetria criaria estado a mais sem nada para dizer.
- **O relançamento relê a configuração ATUAL, não a do nascimento.** O `jev` preserva a escolha
  original lendo o `/proc` do processo que vai morrer, porque houve uma escolha por sessão a
  preservar. Aqui não houve: uma sessão relançada deve refletir o que está ligado agora.

Continua valendo, porém, que a variável entra no ambiente quando o processo SOBE: ligar não muda
sessão viva, e desligar também não. O texto do interruptor diz isso antes de qualquer outra coisa —
sem essa frase a pessoa liga, olha a sessão aberta, não vê efeito e conclui que quebrou.

Isto **não** reabre o que está em [harnesses.md](harnesses.md): lá a decisão é sobre o HANGAR usar
function hooks para ler estado de sessão, e ela continua de pé. Este interruptor é o portão para o
plugin de quem usa.

## Documentos ativos sem acesso ao token

22/09/2026. O visualizador usava uma URL com o token principal e `allow-scripts`: o HTML podia
ler a própria URL mesmo sem `allow-same-origin`. A abertura em nova aba também perdia o sandbox.
Arquivos citados e uploads agora usam `file_response`: HTML/XHTML ficam num iframe com URL `data:`,
sem mesma origem nem referrer, dentro de uma página confiável. A codificação base64 é transmitida
em blocos; SVG/XML continuam como arquivos, com scripts bloqueados por CSP. O ETag dos arquivos
citados mudou para invalidar respostas antigas na revalidação; cache fresco anterior dura até 60s.
No navegador embutido, o HTML executou JavaScript, mas não leu token pela URL, baseURI ou referrer,
nem acessou a página pai ou o armazenamento. Sem mudanças na autorização de caminhos.

## Configuração compartilhada: leva o conteúdo, o destino resolve caminho e programa

(25/09/2026, pedido do usuário.) Levar a configuração de uma máquina para outras, só manual.
Decisões dele, que o desenho não pode amolecer: quem envia vence; segredos vão (variáveis `env`,
cabeçalhos de MCP, motores, chaves de voz), porque as máquinas são internas e o pacote viaja pelo
Tailscale; link de skill é do layout de uma pessoa, então vai o conteúdo; arquivo que um hook usa
e não existe no destino vai junto; caminho absoluto é resolvido pelo Hangar do destino, e não por
troca de texto, porque o destino pode ser Windows.

Os marcadores são `⟦HOME⟧`, `⟦CLAUDE⟧`, `⟦CODEX⟧` e `⟦HANGAR⟧`: `{HOME}` colidiria com `${HOME}`
de script de shell e com f-string de Python, e o destino trocaria o que não é caminho. O destino
só resolve marcador em arquivo que a origem marcou. Criptografia extra do pacote foi descartada:
o bearer que vai na mesma requisição abre a máquina inteira, e o Tailscale já cifra o caminho.
Quem leva o pacote é o navegador (ele tem o token de todas as máquinas), então nenhuma máquina
precisa conhecer a outra pelo `peers.json`.

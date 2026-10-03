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
  - **Protocolo só pro recém-chegado** (`snap[m] is None`), sem lista de membros; veterano não
    recebe nada. Adicionar o 5º membro disparava 5 prompts de 1,5KB, 4 redundantes. Em 26/09/2026
    o "fulano entrou" também saiu: na native-parity os árbitros receberam 269 avisos do painel
    (175 entradas, 67 saídas), que viraram 64 turnos próprios, 54 sem efeito, e a lista de membros
    envelhecia a cada troca de sessão. O grupo passou a ser consultado (`sessions` com `grupo`,
    rodapé `# seu grupo:` do `--list`). Grupo com `orq: true` no sidecar (`--pair --orq`, e o
    vigia) não recebe nem o protocolo: o comum mandava falar 1:1 livre, criar `grupo-<gid>.md` e
    trocar de branch, contra o kick-off; o hook reinjeta uma frase que aponta pro kick-off.
  - **Tarefa diferente da existente é 409** sem `--substituir-tarefa` — cada `--pair` de um árbitro
    sobrescrevia a de todos, calado.
  - **Saída não avisa os locais que ficaram** (26/09/2026, mesma medição acima): recado pra quem
    saiu volta "sessão não encontrada". Ficou o risco de uma sessão nova com o nome reusado receber
    recado dirigido à antiga. Remoto continua por `/unpair-remote` no unpair e no kill, senão o
    sidecar de lá fica órfão; na varredura só loga (rede dentro do `list()` não).
  - **Varredura de morto fora do app roda no fim de `list()`, e três coisas a seguram:** contador
    DE CLASSE (há 4 instâncias de `SessionRegistry` — api, sse×2, prune — e todas chamam `list()`);
    ausência confirmada por **tempo** (`_PAIR_AUSENCIA_MIN_S`), não por número de polls, porque
    `kill()` e `rename()` chamam `list()` numa janela em que o nome está ausente de propósito; e lista
    vazia = tmux fora = não varre, senão dissolvia todo grupo da máquina. O dict de classe (`_pair_ausencias`) é
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

## Grupo de 1 só existe no grupo `orq` com execução `auto` viva

29/09/2026. Na `orquestrar-auto` o árbitro lança sozinho (`hangar-send --pair --orq` sem par) e
quem abre o time é o orquestrador; entre uma Task e outra o grupo volta a ter só o árbitro. Pela
regra antiga, grupo de 1 se desfazia e arquivava o `regras-<gid>.md`, que o orquestrador relê a
cada kick-off. Por isso o `leave()` segura o último membro enquanto `runs.group_phase(gid)` diz
`live`. "Viva" é o `orq.json` com `auto` e sem `execucao_fim`, não o batimento do vigia: o vigia
roda com `Restart=always` e o reinício deixa uma janela sem batimento, que dissolveria o grupo no
meio da execução. O outro lado: sem peers e sem execução viva, o hook de `SessionStart` (que lê o
arquivo do sidecar, fora do backend) reinjetaria o protocolo para sempre. A varredura do `list()`
(`pair.dissolve_lone_orq`) apaga o sidecar e arquiva o contrato quando a execução acabou, ou
quando nenhuma começou 1 h depois da última escrita do sidecar (`ORQ_LAUNCH_GRACE_S`): entre o
`--pair --orq` e o `execucao_inicio` o árbitro escreve o contrato e roda o `orq init`, e dissolver
ali trocaria o `gid` já anotado.

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

## Triagem de recados da orquestrar-auto: Jev ligado, regex só anotando

Na `orquestrar-auto` o `orq notify` sem `[aviso]`/`[decisao]` passa por uma triagem que pode
descartar o recado em vez de acordar o árbitro (o descarte fica na linha do tempo, com o texto
inteiro). O lançamento usa `--jev on --regex shadow`: o Jev descarta; a regex, usada só sem chave
do Jev, apenas anota "teria descartado" até o usuário conferir e ligar.

- **Regex** (`orq_triage.py`, porte de `~/.hangar/orq/jev-calibracao/regex/rx.py`, 28/09/2026):
  690 recados classificados (416 da calibragem 26/08–25/09, fora 1 excluído, + 274 da
  native-parity conferidos um a um por três subagentes), 232 descartados, 0 descarte de recado
  que precisava do árbitro. Os 18 casos achados na conferência serviram para ajustar os vetos,
  então não sobrou base cega: por isso nasce anotando. O `labeled.json` virou teste
  (`test_orq_triage.py`): mudança na regex que descarte um `wake` falha.
- **Jev** (29/09/2026, commit `648c1531`): os 416 da calibragem (149 wake, 258 no_wake, 9
  ambíguos), 2 sorteios (832 chamadas, 0 erro), perguntas e limiares atuais do `orq.py`
  (`DISCARD_P` 0,85, `VETO_P` 0,40), OpenRouter `typesafe/jev-1.13-20260917`. Descarte errado =
  rótulo `wake` descartado: 0 nos dois sorteios; descartou 85/258 (33%) e 87/258 (34%) dos sem
  ação, nenhum ambíguo. Zero erro era a condição da spec (§2.4) para nascer ligado
  (`AUTO_JEV_DEFAULT = "on"`).

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

## Compartilhar sessão: a porta do convidado é a única na internet

(28/09/2026, pedido do usuário.) O convidado tem Hangar e recebe a sessão como um servidor a mais
("Convite · dono"). Dentro dela pode tudo, e isso equivale ao usuário do sistema do dono: a sessão
roda em `bypassPermissions`, o terminal é um `tmux attach` completo (`switch-client` alcança as
outras sessões) e `/file` lê o que a conversa citar. O filtro de rotas delimita a interface, não é
fronteira de segurança.

"Gerar link local" (29/09/2026, pedido do usuário): na mesma rede, sem Tailscale, o link sai
`http://<ip-da-rede>:8766/convite/<código>`. A 8766 escuta em `0.0.0.0` quando o backend escuta fora
do loopback; o portão reconhece o convidado pela PORTA local, então vale igual pelos dois caminhos.
O convite local fica marcado (`local`) e não liga o Funnel. Página e resgate devolvem o endereço
por onde o convidado chegou (`scope["server"]`: loopback = Funnel, senão o IP da rede). Os
clientes só aceitam `http` num convite quando o host é IP privado (10/8, 172.16/12, 192.168/16).

App na porta do convite (30/09/2026): com `CP_PORT=8766` o portão tratava todo pedido como de
convidado e respondia 401 até ao dono e ao `/api/peers/ping`; a VPS recusou 21 deploys seguidos
por isso. Com as portas iguais (`share_tunnel.port_clash()`) o backend não abre a porta do convite,
o portão não filtra e gerar convite responde 409 `erro_compartilhar_porta_do_convite`: o Funnel da
8766 exporia a API inteira do dono.

O Funnel expõe só `127.0.0.1:8766`, em `:8443` (a 443 é o `serve` da tailnet e a 10000 é a prévia
de porta do `tunnel.py`; Funnel só aceita essas três). Nessa porta só vale o token do convidado
(Bearer ou `?token=`); o token do dono e o cookie são recusados. Abrir `/convite/<código>` não
gasta o código, porque prévias de link (WhatsApp) fazem GET.

O dono responde 410 ao revogado, e no servidor de convite o 401 também vale como "encerrado":
o dono apaga registros revogados depois de 30 dias e pode perder o `shares.json`. Em nenhum dos
dois casos o app apaga o servidor nem abre a tela de login (401 comum é login perdido). O 503
`erro_sessao_indisponivel` é tentar de novo: o dono trocando o modo da sessão, o tmux sem
responder ou o túnel fora na hora do resgate (o código não é gasto).

Revogar corta SSE e WebSocket abertos em cerca de 5 s (WebSocket fecha com 4410). O convidado
nunca conta como o app aberto do dono (o dono continua recebendo push) e a lista dele esconde o
nome das outras sessões (campos `pair`/`then`). O nativo guarda uma entrada por endereço de dono
e resgata cada convite novo mandando o token que já tem: o backend acrescenta a sessão a esse
token (01/10/2026, pedido do usuário; antes o mais novo substituía o anterior). Endereço que já é
servidor próprio recusa o resgate. PWA e app Expo seguem com um convite por endereço.

## Par externo: sessões de pessoas diferentes

(01/10/2026, pedido do usuário.) Duas pessoas, cada uma no seu Hangar e na sua tailnet, pareiam
uma sessão de cada lado: as sessões trocam recados, cada pessoa só LÊ a sessão do outro no app
nativo. Transporte é Tailscale Funnel nos dois lados (8766 em `:8443`, como no compartilhamento);
a ponte na VPS (`app.hangar.dev.br`) ficou para depois, assim como PWA e app Expo.

**Um token de convidado cobre várias sessões.** Vários `Share` dividem o mesmo `token_hash`;
`POST /api/guest/redeem` aceita `token` e, se ele ainda vale, a sessão nova entra nele. O porteiro
guarda o conjunto de sessões do token, cada uma com o seu `kind`, e a lista e o SSE da lista
filtram por esse conjunto, relido a cada pedido. Revogar tira só aquela sessão. `attach` copia os
registros do `peer_token` apontando para o registro raiz (`parent_id`): revogar a raiz revoga as
cópias. As duas partes nasceram do mesmo pedido: o par só aparece na mesma entrada "Convite · dono"
se um token puder cobrir uma sessão de compartilhamento e uma de par.

**Por que o token de entrada é um `Share` de `kind: "pair"`.** Herda sem código novo o que a
revisão crítica listou como o que daria errado numa cópia: o Funnel ligado enquanto houver
registro ativo (`sync_tunnel`), vida da sessão (`life`/`set_life`), `rename`, revogação ao fechar,
`sweep`, o vigia de stream e o 410 por 30 dias. O `kind: "pair"` só alcança leituras:
`events`, `history`, `commands`, `plan-preview`, `subagents`, UM arquivo de `uploads`,
`transcript-image`, mais `POST /api/pair/message` e `DELETE /api/pair`. `file`, terminal, git,
`/input`, `pair-invite` e `pair-accept` dão 403 (os dois últimos também para convidado de
compartilhamento, senão ele criaria um token de par que sobrevive à revogação). "Parar de
compartilhar" revoga só `kind: "share"`; fechar a sessão revoga todos.

**Por que `peers.json` não recebe par externo.** Ele guarda o token de DONO das máquinas do próprio
usuário e é sincronizado com os apps; um token de terceiro ali iria parar em todos os aparelhos. O
registro de saída é `external_pairs.json` na pasta dos vínculos (`pair._pair_dir()`), 0600, com
o `peer_token` em claro porque é ele que chama o outro lado. Alias que colide com um id do
`peers.json` responde "ambíguo" e nunca cai no `peers.json`; o `hangar-send` tenta o par externo
primeiro e só volta ao `peers.json` com 404 sem código, `erro_par_inexistente` ou 405 (backend
mais antigo que a rota).

**Só 410 desfaz o par.** 401, 403, 5xx e rede fora do ar dão erro a quem mandou e mantêm tudo; 429
e 503 passam como estão. Arquivo ilegível vira lista vazia no `share_store`, e desfazer no 401
derrubaria pares por causa de um arquivo corrompido do outro lado.

**O endereço é restrito, porque o resgate é aberto na internet.** O host só vale como
`https://<labels>.ts.net:8443` (alfabeto estrito dos rótulos): o backend faz a chamada com o
token do usuário, então endereço livre seria SSRF. Chamada à outra máquina NUNCA segue redirect
(`Authorization` iria junto para onde o 3xx apontasse). O nome da sessão do outro lado entra num
comando `hangar-send` do protocolo injetado, por isso só `[A-Za-z0-9._-]{1,64}`; o `owner` vai
slugado.

**Quem assina é o token, nunca o texto.** O backend de destino escreve
`[de fora: alias::sessao]` com o alias gravado no resgate, neutraliza toda linha que comece com
`[de`, `[painel:` ou `[grupo:` (inclusive depois de espaço e de caracteres de largura zero, que
o modelo ignora ao ler), corta em 16 000 caracteres e reaproveita o anti-loop do grupo. O
protocolo manda tratar o recado como pedido de terceiro (não apagar, não commitar, não mexer em
configuração nem credencial) e `--aceitar-par` só vale com link que o usuário colou: link que
chegou em recado nunca é aceito. A sessão segue no modo de permissão que tinha, inclusive bypass.

**Nativo.** A entrada "Par · máquina" que só tem sessão de par fica em memória, nunca em disco,
e é refeita ao abrir o app. Convite de compartilhamento resgatado depois a torna persistente e
o par passa a se anexar ao token persistente. Os pares vêm de TODAS as máquinas próprias, não só da
ativa, e entrada de servidor cuja leitura falhou não é descartada. O só leitura é por sessão
(`guest_kind`): o app não chama rota que o porteiro recusa (custo, git, atalhos, runners,
terminais de atalho, modos de permissão).

**Limite conhecido.** Codex, Pi e Kimi perdem o protocolo externo depois de `/clear`: quem o
reinjeta é o hook de SessionStart, que só existe no Claude (o protocolo de par comum já se
comporta assim).

## Rede local antes do Tailscale: `baseUrl` é identidade, a rota é `baseOf`

(29/09/2026, pedido do usuário.) A mesma máquina é alcançável pela rede local (no trabalho, a
delphi-02; em casa, o PC de casa) e pelo Tailscale, e o caminho local evita a volta pelo relay.
O servidor informa o próprio endereço local em `GET /api/peers/identificador` (`lan_url`, vazio
quando o bind é só loopback). O cliente guarda `lan: {url, id}` na entrada e, a cada conexão da
lista, testa o local com prazo de 800 ms por `GET /api/peers/prova?desafio=`, SEM credencial: a
máquina devolve o HMAC-SHA256 de `desafio|identificador` com o token. Só usa o local se a prova
bater e o identificador for o mesmo. O mesmo `192.168.x.y` em outra rede é outro aparelho, e
mandar o token antes da prova o entregaria a ele (achado da revisão); o identificador sozinho não
basta porque o token é igual em todas as máquinas do usuário. Conexão que cai esquece a rota, e é assim que a troca de rede é percebida.

Provar a identidade não basta para ganhar: o local só vence se provar ANTES de o principal
responder (os dois são perguntados juntos). Medido em 29/09/2026, de casa: o `192.168.77.142` da
delphi-02 respondia pela VPN do trabalho (`wg0`) em 412 ms para 5 chamadas, contra 309 ms pelo
Tailscale — "sempre o local" escolhia o caminho mais lento.

`baseUrl` segue sendo a identidade (dedupe, sincronização, agrupamento, diário, endereço mandado
ao par). Chamada nova sai por `baseOf(s)` ou pelo `getBaseUrl` do `ApiEnv`; `s.baseUrl` cru numa
URL de chamada ignora a rede local sem erro nenhum.

Página HTTPS (PWA pelo `*.ts.net` ou pela VPS) não tenta o `http://` local: o navegador bloqueia
conteúdo misto. Valem o app nativo, o desktop nativo e o Electron (página em `http://127.0.0.1`).
A máquina precisa escutar fora do loopback (`CP_LAN_BIND_IP=0.0.0.0`); o token continua exigido.

## Atalho No Hangar: cópia única por atalho, fora da sessão

(29/09/2026, pedido do usuário.) Cada atalho tem duas formas. "Na sessão" é a antiga: o terminal
pertence à sessão que clicou e morre com ela (`registry.kill` → `close_all`). "No Hangar" roda UMA
cópia no servidor inteiro, com dono vazio e a opção `@cp_shortcut_key` no multiplexador; `close_all`
e a lista de uma sessão não a alcançam, e clicar de novo reaproveita a cópia viva. A aba dela
aparece no painel de terminal de toda sessão.

Por que não uma cópia por sessão: a VM DELPHI-02 aceita uma conexão RDP por usuário, e a segunda
derruba a primeira (`ERRCONNECT_CONNECT_CANCELLED` no terminal do atalho anterior). Fechar a sessão
que clicou também levava junto RDP e túnel.

O estado chega pelo evento `shortcut_terminals` do stream da lista, lido em paralelo com a lista de
sessões para que um tmux travado nunca atrase `sessions`; nunca há um SSE por terminal. Convidado
não recebe o evento.

**Pergunta.** O app oferece "responder pelo app" quando a linha do cursor termina em `:` ou `?` E o
processo em primeiro plano dorme esperando leitura do tty (Linux). Linha que quebra em várias linhas
da tela é reunida de volta (linhas que preenchem a largura do pane).

Medido no Linux (CachyOS, kernel 7.1): o `read -p` do bash de um atalho dorme com `wchan` =
`wait_woken`; o `sleep` dorme em `hrtimer_nanosleep` (medido em 29/09/2026 num
`tmux new-session 'sleep 60'` descartável, lendo `/proc/<pid>/wchan` do filho do shell).

No Windows (psmux) a regra é tela + CPU parados, e o `.cmd` que grava o código de saída tem regras
próprias: medição, o que falta medir e a regra de `%` em
[windows.md](windows.md#terminais-de-atalho-no-psmux).

**Limite da detecção.** A pergunta é lida do tty (`wait_woken` / `n_tty_read`): programa que espera o
teclado por `poll`/`epoll` (o `read` do fish, o readline do Node/inquirer) não vira pergunta. Incluir
`ep_poll`/`do_select` traria perguntas falsas de qualquer programa de tela cheia, então fica de fora.

## Orquestrador sem LLM: a conversa e o painel saem dos arquivos da execução

### Histórico independente das sessões

30/09/2026. O histórico lista as pastas do cofre mesmo depois de a sessão orquestradora sair
da lista. `GET /api/orq/{id}/panel` reaproveita o painel por execução; não cria sessão fictícia
e continua só de leitura. Rust abre a consulta pelo relógio da barra superior; o PWA mantém
sua tela Orquestração. Nome vem do título do plano, com projeto e caminho como identificação.

O fluxo de escrita (`orq init`, início de Task e encerramento) guarda `plan.snapshot.md` para
preservar títulos e tarefas quando a pasta original for removida. Consultar não cria essa cópia.
Execuções antigas sem plano preservado usam os dados ainda disponíveis, sem inventar tarefas.

Consumo de execução encerrada ignora sessões vivas de mesmo nome. Claude e Codex são cortados
por resposta entre início e fim, antes da agregação e da deduplicação; retomadas posteriores
não aumentam o histórico. Fonte ausente ou sem leitor histórico preciso aparece como consumo
parcial. Os registros da execução e os transcripts continuam sendo as fontes: apagar um
transcript pode tornar sua medição indisponível.

### Base do painel (29/09/2026)

A sessão `orq` não tem transcript de modelo; o que ela mostra é
lido dos arquivos da execução por um parser só, `orq_timeline.py`, e cada cliente desenha o
resultado. O `ChatEvent` ganha o campo `orq` (estrutura da linha) e mantém o texto cru, para o
cliente que ainda não conhece o campo (o app Expo) seguir mostrando o aviso de antes.

**Uma rota por execução, sem SSE por card.** `GET /api/sessions/{name}/orq/panel` devolve um
retrato da execução: Tasks, Time, Decisões, Automação, Consumo e Integração. A pasta vem da linha
em cache da lista de sessões; `runs.find` relê todas as execuções e some no reinício do vigia. O
`orq` está em `_BLOCKED` do `share_gate.py`, então o convidado não alcança a rota. O `GET` nunca
escreve na execução (o índice sqlite de custos pode ser atualizado pelo módulo de custos que já
existia). O retrato só é refeito quando um arquivo da execução muda; o estado ao vivo de cada
sessão do Time vem da lista de sessões que o cliente já mantém (`sessionsStore`), não do retrato. `automation.mode` é texto (`"auto"`, não número), e uma leitura
que falha vira item de `errors`, nunca exceção.

**Recado do `notify`, a linha e o Jev.** O `notify` grava o Jev, envia (teto de 30 s no
`orq.py`) e só então escreve a linha; por isso o parser pareia linha e Jev numa janela de 45 s
(`JEV_MATCH_S`) sobre o arquivo inteiro. Papéis, fechamentos e integração vêm do próprio
`orq.py`, por `orq_start._orq()`, e não de uma segunda leitura reimplementada.

**As quatro gravações novas do `orq.py`, e o que cada uma resolve:**

- `from` na linha do `notify`: o remetente não se deriva de nada depois (`vigia` no alarme; o
  nome de quem chamou pelo `hangar-send --whoami`; `null` na dúvida, nunca um palpite).
- Linha `advance` "T{n} entregou a rodada k · <commit>": o mock mostra a entrega e nenhum
  evento existente vira essa frase.
- `probs` na resposta do Jev: a probabilidade de cada escolha não era gravada, só a escolhida.
- `sessions.jsonl`, uma linha por sessão aberta com os ids (Claude: `session_id` e `config_dir`;
  Codex: `thread_id` e `codex_home`): o transcript de uma sessão fechada não se acha sem id, e
  o rollout do Codex só nasce no primeiro turno, então o caminho não serve. Gravar dado extra
  nunca trava a orquestração viva: falha vai para o diário e o passo segue. Nome repetido: a
  última linha vence.

**Consumo em andamento.** Soma os transcripts do time pelo índice de custos do Hangar, só do uso a partir de
`execucao_inicio` (no Claude o corte é por segmento diário do índice, não por resposta). Cada
transcript é achado por, nesta ordem: ids de `sessions.jsonl` (execuções novas), `medicao/*.json`
e a sessão viva de mesmo nome; o que nenhum caminho alcança entra em `sessions.missing` e o
painel mostra "M de N sessões". Cache de 60 s (5 s quando algum transcript não pôde ser lido
agora). **A soma nunca faz um poll esperar:** quem chega com ela em andamento recebe o último
valor guardado, ou `None` se ainda não houve nenhum (no primeiro poll a tela diz "Somando os transcripts do time…").
Tokens não são preço nem cota; o total marca `usd_partial` quando algum modelo não tem preço.

**Estado da Task.** Veredito `aprova` ou `corrige` vira "aprovada · Rk" (k = rodada do
veredito), e só `integrada` fecha a Task; `reprova` e `devolvido` viram "reprovada · Rk".

**Medido em 29/09/2026** (execução `2026-09-29-cad3e6fe`, `orq_timeline.panel` chamado em
processo contra a pasta real, não por `curl`: o backend de uso ainda não tinha a rota): o
primeiro retrato levou 0,107 s e o seguinte 0,002 s. Das 23 sessões do time, 7 tiveram transcript
achado (pela `medicao/` e pelas sessões vivas; a execução começou antes do `orq.py` gravar
`sessions.jsonl`) e 16 ficaram em `missing`. O total mostrado, 13,17 dólares, é parcial nos dois
sentidos: `usd_partial` verdadeiro e 16 sessões fora. Não medido: `time curl` pelo backend de uso
e execução `auto` nova com as quatro gravações.

**Paridade.** Web e nativo têm a linha do tempo e a aba Orquestração; a folha Orquestração do
web (celular e desktop estreito) ainda não existe no nativo, e o app Expo não tem nada disto. O
estado de cada um está em `desktop-native/docs/chat-parity.md`.
## Time da orquestração pertence ao trabalho atual

Em 29/09/2026, a configuração do app antes do grupo lia `regras-padrao.md`; trocar conta/modelo
preservava seus nomes. O planejador reproduziu os nomes globais no contrato cad3e6fe.
Nove nomes distintos tinham prefixo de outro contexto: árbitro mais quatro executores e quatro
revisores, contados nos eventos task_inicio T1–T4 e na identidade real do árbitro. Isso é um
incidente, não nove execuções. O usuário pediu usar somente o time configurado para o trabalho.

O editor e o LLM usam GET/POST `/api/sessions/{name}/orq`. Antes do grupo, o Markdown pertence
à identidade atual; uma sessão recriada com o mesmo nome não importa configuração anterior.
GET devolve `grouped`, `session_prefix` e `session_identity` para os clientes distinguirem
rascunho/grupo e isolarem edições. A criação de grupo orq promove o registro do fundador;
árbitro novo separado associa o registro de origem por POST `/orq/grupo` com gid e mtime.
A origem passa a apontar ao mesmo contrato; há uma tabela editável, sem template global.
Conflitos de versão/contrato respondem 409; edição externa não é sobrescrita pelo rollback.

CLI e backend usam a mesma identidade stdlib: chave do cano ou vida do multiplexador.
O script registra identidade do árbitro e dos papéis nos eventos, e a leitura de uma execução
viva exige identidade compatível. Re-init não preenche identidade ausente de um registro
legado pelo nome atual. Legado com pareamento real continua acessível; ausência de prova não
autoriza atribuir outra sessão. Codex legado sem chave/pane usa a thread disponível, então
seu rascunho pode mudar no /clear. Nomes novos são do contexto, escolhas existentes preservadas.

## Fechamento das sessões concluídas na execução automática

Na mesma data, `orq done` oferecia executor de Task fechada, porém só oferecia seu revisor
quando a execução inteira acabava. Consulta somente leitura com a função corrigida sobre
cad3e6fe passou a listar os revisores das Tasks 1–5 fechadas, incluindo os quatro nomes
originais. Nenhuma sessão foi encerrada nesta conferência.

O automático é `vigia.sh -e`: roda `orq advance --detach` e consulta `done` a cada ciclo.
`advance` não fecha sessões diretamente. Agora `done` oferece executor e revisor concluídos;
quem tem Task aberta e o árbitro atual ficam fora. O vigia mantém 600 s de inatividade,
proteção de subagentes, servidor remoto e três falhas de fechamento com aviso. Confere grupo
e identidade registrada antes de fechar; não alcança pessoa movida/recriada por nome antigo.

As referências antes trocavam/desarmavam o vigia nas etapas finais, podendo deixar os últimos
pares sem completar sua janela de limpeza. O vigia da execução permanece até os candidatos
terem fechamento conferido. Revisão final/retrospectiva são fechadas explicitamente pelo nome
aberto, após entregar e sem trabalho em voo; seus monitores avulsos não substituem a limpeza.

Conferências desta alteração: revisão estática independente, sintaxe Python/Bash, diff e
seleção readonly de candidatos reais. Regressões foram escritas, não executadas. Fluxos do
app, compilação e fechamento real ainda não conferidos; mudanças estão na worktree
`/home/jefferson/Projetos/hangar-orq-team-context`, não instaladas no serviço ativo.

## Connect: a porta dele nunca é local

O Hangar Connect publica a máquina em `<maquina>.<conta>.hangar.dev.br`: o traefik da VPS repassa
por SNI sem abrir o TLS, o `frpc` traz os bytes até o Caddy local, que abre o TLS e encaminha para
o backend. Medido em 2026-10-02, antes da porta própria: o Caddy fala de `127.0.0.1`, então
`/api/desktop/palette` (só-local) respondia **200** pela internet, inclusive com
`X-Forwarded-For` forjado — todo acesso de fora valia como o dono na máquina, sem limite de
tentativas. Com a porta `127.0.0.1:8768` e o `ConnectPortGate` trocando o cliente por
`192.0.2.1` (RFC 5737) por fora de todos os middlewares, a mesma chamada passou a **403**.

A decisão é pela porta do socket, como a do convidado (8766): o `proxy_headers` do uvicorn vale
para todos os sockets do processo, então cabeçalho nunca serve para decidir. O limite de
tentativas pelo Connect é o bloqueio de sempre (8 erros / 30 s) num contador próprio; atraso por
tentativa não serve (o atacante abre conexões em paralelo, ver `auth.py`). O acerto do dono pelo
Connect não zera esse contador: o app dele acerta a cada poucos segundos e daria ao atacante 7
palpites novos a cada acerto. Custo aceito: quem martelar a senha trava o acesso pelo Connect por
30 s; LAN e Tailscale seguem livres.

## Cookie de login só lê

Todas as máquinas do Connect e o `app.hangar.dev.br` são o mesmo site para o navegador: a página
servida por uma máquina dispara pedidos a outra levando o cookie dela (`SameSite=Lax` não
separa subdomínios) e consegue gravar cookie com `Domain=hangar.dev.br`. Um segundo domínio não
resolveria — as máquinas seguiriam no mesmo site entre si. Por isso o `cp_token` só autoriza
`GET`/`HEAD` (ação exige a senha no cabeçalho ou na query, que outra página não tem), e em https
só vale o `__Host-cp_token`, que outra máquina não consegue gravar; o PWA grava o prefixado e
apaga o antigo. O sync recusa ação com `Sec-Fetch-Site: same-site`/`cross-site`. O `cp_sync`
mantém o nome por ora: o `hub()` do app nativo só guarda `Set-Cookie` começando com `cp_sync=`.

## hangar-server: a porta pública em Rust, o Python atrás

O serviço continua subindo `python -m app.main`. Com o binário e sem `CP_RUST_SERVER=0`, o
uvicorn escuta numa porta livre de `127.0.0.1` e o `hangar-server` assume a porta pública como
filho (`app/rust_server.py`), com a porta interna, o token, a lista `forwarded_allow_ips` do dono
e um segredo novo a cada subida. O segredo vai só no ambiente do filho; no Python ele mora na
memória de `internal_api` (`set_secret`), porque no `os.environ` vazaria para toda sessão que o
backend sobe. O filho morre com o Python por um mecanismo só, em Linux, Windows e macOS: o
Python segura o stdin dele como cano, e o binário sai quando o cano fecha. Sem `preexec_fn`, que
não é seguro com threads vivas. No Windows o `Restart-HangarTask` reconhece o `hangar-server.exe`
filho do backend: sem isso, a porta "de outro processo" barrava todo reinício.

A reserva é no mesmo processo, sem novo lifespan: dois lifespans rodariam watchers e hooks em
dobro. O motivo vai ao diário como `hangar_server.reserva` (`sem_binario`, `sem_resposta`,
`protocolo`, `quedas`, `erro`, `porta_ocupada`); cada queda, como `hangar_server.caiu`. A saúde
traz `protocol`, e o Python só aceita o mesmo `RUST_SERVER_PROTOCOL`: a `server-latest` é sempre a
mais nova, e uma máquina atrasada pode baixar um binário que fala outro contrato interno. Com o
Rust na frente, todo pedido chega ao uvicorn interno por `127.0.0.1`: o `forwarded_allow_ips`
dele sempre inclui esse endereço, senão a LAN passaria por loopback e escaparia do limite de
tentativas. O Rust põe no `X-Forwarded-For` o cliente já resolvido, nunca o que veio de fora.

Os binários vêm da release `server-latest` para `~/.hangar/bin/`, pelo instalador, pelo botão
Atualizar (`_preparar`) e pelo passo `2026-10-02-hangar-server-binarios`; falha vira aviso.

### O contrato interno é versionado à mão

Não há comparação de commit entre o binário e o Python: a `server-latest` acompanha a main, e
uma máquina atrasada pode ter um `hangar-server` mais novo que as rotas `/internal` dela. Quem
barra isso é o número. Qualquer mudança nas rotas `/internal`, no conjunto de eventos do
`side-events` ou nas variáveis de ambiente passadas ao filho sobe `RUST_SERVER_PROTOCOL`
(`backend/app/rust_server.py`) e `INTERNAL_PROTOCOL` (`crates/hangar-server/src/lib.rs`) juntos,
no mesmo commit. Subir só um dos dois faz o Python recusar o binário e assumir a porta (visível
no diário); não subir nenhum não dá aviso: o Python aceita um binário que fala outro contrato, e
o defeito só aparece no comportamento. O `hangar-cano` segue o mesmo raciocínio com
o `versao` do snapshot, que acompanha o `VERSAO` de `cano.py`
(`backend/app/adapters/claude_headless/cano.py`; `VERSION` em `crates/hangar-cano/src/protocol.rs`).
O download registra no diário o commit do manifesto, só para diagnóstico.

### Linha de base (02/10/2026, antes da troca)

Fonte: backend vivo (3 sessões, 4 conexões) e benchmarks avulsos em Python 3.14, nesta máquina
(16 núcleos, 31 GB).

| Métrica | Antes |
|---|---|
| Memória do backend | 232–312 MB de RSS, 247 MB anônimos, 20 threads |
| CPU média | ~1,1% de um núcleo |
| `cano.py` por sessão sem terminal | 22 MB de RSS, 6 threads, 22 ms para subir |
| `/history` (Claude 0,9 MB / Codex 3 MB / Codex 35 MB) | 5–8 ms / 15–20 ms / 80–110 ms |
| `/history` completo, Claude 300 MB / `limit=200` | 362 ms / 45 ms |
| Atraso do laço com 8 leituras de histórico em paralelo | p99 9,6 ms, máx. 16 ms |
| Recursos presos por chat aberto | 2 threads do pool do anyio (limite 200) e 2 inotify |

### Depois da troca

Preenchida na verificação manual com o dono, mesma máquina e mesmas sessões da linha de base.
O `MainPID` do serviço é o `uv`, não o Python: o backend é o filho dele e o `hangar-server` é
filho do backend.

- Backend e `hangar-server` (RSS em KB):
  `PY=$(pgrep -P $(systemctl --user show -p MainPID --value hangar-backend.service))`,
  `RS=$(pgrep -f .hangar/bin/hangar-server)`, `ps -o pid,rss,nlwp,args -p $PY,$RS` e
  `grep RssAnon /proc/$PY/status`.
- `hangar-cano`: `ps -o pid,rss,nlwp,args -p $(pgrep -f hangar-cano | head -1)`.
- `/history`, cinco vezes por sessão, mediana (`ls -l` do `jsonl` confere o tamanho):
  `for i in 1 2 3 4 5; do curl -s -o /dev/null -w '%{time_total}\n' -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:8765/api/sessions/<nome>/history"; done`;
  na sessão Claude de 300 MB, sem e com `?limit=200`, se ela ainda existir.
- Chats abertos: com nenhum aberto, `nlwp` do `$PY` e `ls -l /proc/$PY/fd | grep -c inotify`;
  abrir 4 chats (2 sessões × celular e web) e repetir.

Anotar valor e unidade, a sessão de cada `/history`, e o que não deu para medir e por quê.

| Métrica | Depois |
|---|---|
| Backend Python (RSS, anônimos, threads) | |
| `hangar-server` (RSS, threads) | |
| `hangar-cano` por sessão sem terminal (RSS, threads) | |
| `/history` (Claude 0,9 MB / Codex 3 MB / Codex 35 MB), mediana de 5 | |
| `/history` completo, Claude 300 MB / `limit=200` | |
| Threads e inotify do Python com 4 chats abertos, contra 0 abertos | |

## Observação terminal Rust com reserva Python

(03/10/2026, Parte 2C, ensaios isolados.) O `hangar-server` abre uma segunda porta em
`127.0.0.1:0`, no mesmo processo e sob a mesma parada do listener público. Isso cobre também
um bind público em IP LAN específico, que não aceita conexões destinadas a `127.0.0.1`.
A saúde anuncia `terminal_address`; o Supervisor só o usa depois de confirmar o protocolo e
conferir IP literal de loopback e porta válida. Endereço ausente/torto desliga a ponte com
aviso. A porta pública recusa o endpoint terminal e a privada só monta esse endpoint.

`POST /__hangar_server/terminal` confere origem TCP de loopback e segredo interno em tempo
constante antes de ler o corpo. Cabeçalho encaminhado externo, inclusive duplicado ou inválido,
recusa com 404; token do dono/convidado não serve. O corpo tem teto de 16 MiB e prazo de 6 s,
operações tipadas `acquire`, `capture`, `release`, sem comando livre. Corpo inválido
responde frase fixa com 400; falha do controle responde 503, nunca pane vazio com sucesso.
O pool mantém os limites da Parte 2C/Task 2 e a captura exata do alvo que `tmux._pane_target`
resolveu. `acquire` inicial confere o alvo; renovações não recapturam. O cliente anexa com
`read-only,ignore-size,no-output` e `-E`, sem grade auxiliar. Cada rodada confere a sessão/pane
e lê um único `capture-pane`; duas molduras identificam cada comando. Mudança de alvo continua
invalidando a leitura, e panes maiores não pagam um limite de células de outra grade.

A ponte Python usa um opener `urllib` somente HTTP, sem proxy/redirect, em thread dedicada,
corpo/UTF-8/JSON limitados, sem dependência
runtime de `httpx`. Endereço e segredo ficam em memória, fora do ambiente global, e somem
antes da nova geração do filho, na saída e no `stop`, inclusive sem processo guardado.
A configuração recebe geração própria. Cache, captura em voo e análise acompanham provider,
vínculo da conversa, época local, geração da ponte e início da leitura. `/clear` invalida tudo;
mesmo texto e mesmo nome não autorizam reaproveitar uma análise velha. O contexto do produtor
não vaza para quem consome eventos do monitor.

Claude mantém uma lease por monitor e uma por produtor de prévia; a prévia renova mesmo
quando recebe só sidecar. `None` no sidecar cai no pane; `""` publica vazio com markdown/full.
Análise Rust do pane só fornece spinner/texto no mesmo quadro, com markdown/full desligados.
O estado temporal, debounce e a precedência das perguntas/plugin/hooks permanecem no bloco
Python original, sobre o texto já capturado. Não existe outro HTTP para o cálculo puro, nem
coleta antecipada de fatos que esse estado não usa. O cálculo puro Rust permanece como
referência testada contra as fixtures Python, sem operação privada. Permissão, loop, shells,
dedupe, drain e SSE continuam no Python.

Codex terminal não abre observador tmux sem consumidor de captura. Estado, pergunta, prévia
por push e app-server continuam nativos. Kimi/Pi/omp seguem o caminho anterior.
Windows usa captura/reducer Python, sem tentar controle tmux/psmux.

Prova isolada: listener público em `127.0.0.2`, privado em `127.0.0.1` com porta efêmera;
processador privado respondeu 200 e ambas as portas fecharam na parada. Executável fake provou
um PID compartilhado entre dois produtores, renovação sem recaptura e reap na última liberação.
A suíte focada cobre reserva, estado temporal Python, `/clear` em voo com vínculo/texto idênticos,
troca de geração, sidecar vazio, autenticação antes do corpo e eventos Codex durante HTTP lento.
O gerador das fixtures força a referência Python: não compara Rust contra Rust.

`RUST_SERVER_PROTOCOL` e `INTERNAL_PROTOCOL` ficam em 3 após retirar a operação privada `reduce`.
`side-events` permanece igual. Em 03/10/2026 a coordenação com `rust-parte2` combinou 3 nesta 2C
e 4 para o contrato posterior da Parte 2B; não reutilizar 3 para dois contratos diferentes.
Não houve reinício/instalação nem validação no app ou backend vivo. Windows não foi executado.
Uma rodada vermelha tentou leitura real `tmux capture-pane -p -t %8 -S -200` em alvo fictício e
recebeu `can't find pane: %8`; nenhuma conversa foi lida. A guarda dos novos testes passou a
bloquear `_run`/`RUN` antes de I/O e conferir no teardown se alguma chamada bloqueada foi engolida.

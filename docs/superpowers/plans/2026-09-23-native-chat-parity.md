# Paridade do chat nativo

**Última ampliação aprovada:** compositor e barra direita também devem alcançar paridade funcional, mantendo o visual nativo. Inventário readonly em composer-rightbar-inventory.md no diretório operacional; fechar recorte em Tasks específicas após o inventário, sem parar o trabalho autorizado. Papéis atualizados: árbitra Opus5.5/xhigh exclusivamente Claude200-5 ao transferir; executores Opus5.5/medium e revisores Opus5.5/high alternando Jefferson/200-3. Sol atual só encerra Task1. Estas escolhas substituem as tabelas iniciais abaixo.

**Primeira ação após compactação:** ler o registro de retomada `docs/handoffs/native-chat-parity.handoff.md` e `/home/jefferson/.hangar/orq/2026-09-23-native-parity/succession.md`; executar `check-quota.py` desse diretório. Ordem do usuário: aos50% da conta Codex pessoal, passar arbitragem para nova sessão Claude200-5 com claude-opus-5-5/xhigh e cessar Codex; depois somente ClaudeJefferson/200-5/200-3. O monitor independente `hangar-native-quota-watch-20260923.service` avisa aos45%/50% e não deve ser desligado por compactação.

## Contrato aprovado

O usuário aprovou aproximar as funcionalidades do chat atual mantendo o visual próprio do Rust. Sequência: corrigir streaming/piscadas e leitura; ferramentas e pensamento recolhíveis; fila, perguntas/aprovações, anexos e ações de conversa. Terminal, navegador, árvore de arquivos, Board/Canvas e administração geral de sessões/contas/modelos são módulos posteriores. Backend Python existente permanece inalterado. Windows/macOS e benchmark da fase anterior continuam pendentes, sem alegar paridade de plataformas.

Worktree exclusiva /home/jefferson/Projetos/hangar-native-desktop, HEAD55e0d909 destacado autorizado. Dirty tree contém a fase1, preservar integralmente. Escrita de produto somente desktop-native/ e native_* de messages/{pt,en}.json. Sem stage/stash/commit/push, sem testes automatizados até pedido; build, compilação de regressões com --no-run e uso real autorizados. Não alterar serviços/checkout principal/sessões alheias. Provas que enviam/interrompem/respondem usam somente cx-* descartável própria ou fixture sintética claramente separada. Token só em memória; não expor em argv/logs/arquivos.

Method: plano local aprovado. Executes with: executor Opus por Task. Domain skill: impeccable. Route: full.
Executor Task4: native-parity-task4-exec, claude-opus-5-5/medium, Claude200-3 (/home/jefferson/.claude-claude-200-3).
Revisor Task4: native-parity-task4-review, claude-opus-5-5/high, ClaudeJefferson (/home/jefferson/.claude-jefferson), proteção somente leitura.
Coordenação/conferência final: native-parity-arbiter, Opus5.5/xhigh na Claude200-5, desde 2026-09-24T02:07 (sucessão da Astra concluída, Codex cessado). Máximo duas revisões por Task contando final da árbitra; nenhuma revisão da revisão. Um escritor, execução serial. Fechar agentes ao terminar participação, preservar demo fora dos cgroups. Próximos executores/revisores alternam Jefferson/200-3 conforme cota; nenhuma nova Task Codex.

Referência visual: cliente nativo atual aprovado, desktop-native/artifacts/delivery-final.png e imagem do usuário /home/jefferson/.hangar/uploads/hangar-5112ff/01a0cf46-82f7-7760-a186-89f5591ca467/1790212763-de3633.png. Não copiar pixels Electron; reutilizar semântica de interação. Tema/vidro aprovados mantidos. Critério: respostas fáceis de ler, detalhes técnicos progressivos, teclado/foco/cópia funcionais, erro/pending explícitos.

### Task 1: Streaming contínuo e rolagem estável
Status: ready-for-agent
Risk: high
Arquivos: desktop-native/src/{app,chat}.rs, src/api/{dto,sse}.rs somente se causa comprovada; fixture/relatório próprios.
- [x] **Step 1: Reproduzir as piscadas e demonstrar a causa**
Observar fluxo de prévia e renderização em janela GPUI; inspeção estática não basta. Separar eventos do backend de repintura/lista. Capturar sequência curta ou vídeo com amostras temporais, texto crescente, quebras de linha e troca para mensagem final. Usar skill systematic-debugging; não declarar causa sem evidência.
- [x] **Step 2: Corrigir atualização incremental e preservar leitura**
Texto crescente deve permanecer visível, sem desaparecer a cada evento; mensagem final substitui prévia sem duplicação. Ao ler acima, novos eventos não puxam ao fim; voltar ao fim acompanha. Preservar reconexão/identidade/delivery da fase1. Regressão focada compilada, não executada automaticamente.
- [x] **Step 3: Conferir janela real — verificação manual**
Build --locked, fixture progressiva para reprodução determinística e sessão própria real quando necessário; registrar diferenças e limites. Não tratar somente print da resposta final como prova do streaming. Entregar relatório/hashes ao revisor e congelar.

### Task 2: Leitura e ferramentas com detalhes progressivos
Status: ready-for-agent
Risk: high
Arquivos: apresentação Rust, traduções native_*, relatório.
- [x] **Step 4: Mapear e apresentar os tipos da conversa**
Inventário de comportamento dos componentes MessageList/ToolGroup/ToolCard/ThinkingBlock e DTOs existentes, usando svelte-code-writer para leitura pertinente. Manter mensagens de usuário/assistente distintas; ferramentas/raciocínio recolhíveis por identidade estável, estado/erro/resumo útil, detalhes e cópia acessíveis; associar resultado ao tool_use_id sem esconder eventos sem par.
- [x] **Step 5: Ajustar leitura sem refazer o visual**
Limitar largura de prosa, Markdown renderizado, listas/código legíveis, conteúdo grande não trava a janela. Foco/teclado nos detalhes; expansão não perde seleção/rolagem. Não mudar protocolo nem truncar irreversivelmente texto/ferramentas.
- [x] **Step 6: Conferir interação — verificação manual**
Conversa mista com texto, raciocínio, várias ferramentas, erro e resultado longo; abrir/recolher/copiar; conteúdo vivo e histórico/paginação; build e prova visual. Relatório de equivalências e diferenças deliberadas.

### Task 3: Fila, perguntas e aprovações
Status: ready-for-agent
Risk: high
Arquivos: cliente HTTP/DTO, estado e controles Rust, traduções.
- [x] **Step 7: Mapear contratos das ações existentes**
Conferir endpoints e payloads atuais de fila, opções, AskUserQuestion e aprovações por provider. Reutilizar contrato existente; não tratar pergunta como texto simples ou inventar aprovação implícita. Registrar capacidades/diferenças no inventário.
- [x] **Step 8: Implementar interações nativas equivalentes**
Perguntas com opções/múltipla escolha/texto, identificação do pedido e envio explícito; aprovações por ações suportadas sem padrão permissivo. Fila com estados e ações oferecidas pelo backend. Sucesso, pending e falha claros; sem repetir mutações em timeout; ignorar respostas atrasadas após mudança de sessão. Preservar rascunho.
- [x] **Step 9: Conferir somente em sessão descartável/fixture — verificação manual**
Clique/teclado, cancelamento, recusa HTTP, pedido que some/muda, sessão trocada enquanto envia e incerteza. Nunca responder aprovação ou apagar fila de sessão do usuário. Documentar casos sem infraestrutura real e diferença da fixture.

### Task 4: Anexos, ações da conversa e entrega
Status: ready-for-agent
Risk: high
Arquivos: cliente HTTP/DTO, compositor/apresentação Rust, traduções e docs.
- [x] **Step 10: Mapear e concluir ações do compositor/conversa**
Inventariar ações do chat existente antes de dizer paridade, incluindo comandos do compositor retornados por /commands (argumentos preenchem rascunho; destructive exige confirmação). Implementar anexar arquivos/imagens, remover anexo antes do envio, estados de upload/falha, citações/abertura segura de anexos e ações diretamente ligadas à conversa suportadas pelo backend. Usar seletores/abertura do sistema e componentes existentes quando possível. Recursos de voz, serviços externos, administração geral e módulos fora do chat não serão incluídos por inferência; registrá-los separadamente.
- [x] **Step 11: Conferir conversa completa — verificação manual**
Leitura/streaming, ferramentas, fila/perguntas/aprovações e anexo sintético próprio; preservação de rascunho, erro/rede, mudança de sessão e teclado. Sem entrada em sessões alheias. Build e provas só no binário final; testes compilados não executados.
- [x] **Step 12: Registrar paridade do compositor e preparar integração do painel**
Registrar implementado/conferido/pendente por funcionalidade do compositor e conversa, comando de execução, capturas e limitações. Incluir arrastar/colar anexo quando biblioteca suportar, reanexar upload existente, proteger rascunho ao preencher comando, Ctrl+L, Esc com confirmação de interrupção, sugestão por Tab com campo vazio. Limpeza de fila só quando explicitamente pedida, devolvendo pendente ao compositor conforme contrato web. Seguir para Task5, sem declarar entrega global encerrada.

### Task 5: Barra direita e controles completos da sessão
Status: ready-for-agent
Risk: high
Origem: ampliação explícita do usuário para compositor e barra direita. Inventário em /home/jefferson/.hangar/orq/2026-09-23-native-parity/composer-rightbar-inventory.md. Controles por sessão estão incluídos; a exclusão antiga se mantém para administração global de contas/modelos e módulos completos de terminal/navegador/Git.
Arquivos: DTO/API, módulos de status/painel/compositor Rust e traduções, docs.
- [x] **Step 13: Portar estado e métricas da sessão**
Conferir parseStatusLine/StatusFields no core e portar contrato (modelos/esforço, contexto/custo/limites, repo/branch/dirty), sem depender de leitura direta de tmux/JSONL. Consumir stats e campos StateEvent/SessionInfo pertinentes; valores desconhecidos não viram zero/sucesso. GET /cost só quando painel visível e com intervalo equivalente; cancelar ao trocar identidade, não criar SSE por cartão. Regressões do parser relevantes compiladas, não executadas.
- [x] **Step 14: Implementar compositor e barra direita funcionais — verificação manual**
Cobertura de ciclo/planos que fecha junto a estes controles: aprovação Codex de hooks antes de jsonl/tracked=true a partir das opções da lista; implementar plano Claude headless idle em modo plano com mudança de modo/envio e falha parcial explícita; leitura de /plan-preview do Claude com terminal. Esta colocação é explícita após diagnóstico daTask3; não excluir esses casos da entrega.
Compositor recebe seleção de modelo/esforço/permissão/modo plano da sessão conforme provider, via catálogos/rotas existentes e gesto explícito; sem mudar padrões da conta. Painel direito mostra estado, contexto/janela/custo, último turno/atividade, limites, fila, estado de loop e aviso de recarregar. Recolher/redimensionar sem perder leitura. Compactar preenche comando e preserva rascunho; recarregar só em condição permitida e confirmação explícita. Mudanças do projeto: contagem/lista e leitura de diff/arquivo pelos endpoints existentes; sem autooperações Git. Atalhos send_text respeitam confirmação/preenchimento; shortcut-shell só se API ativa suportar e com confirmação quando configurada, nenhuma execução automática. Terminal, navegador, árvore/Git completos, gestão global e voz continuam módulos próprios; listar dependências na matriz, não simular ações inexistentes.
Verificar loading/vazio/erro/sucesso, keyboard/foco, alteração em voo e troca/recriação de sessão. Provas mutáveis só cx própria ou fixture; não trocar modelo/permissão de sessões de trabalho, não fazer operações Git nem shell em repo do usuário para conferir botões. Manter appearance nativa aprovada.
- [x] **Step 15: Entregar paridade verificada e encerrar recursos próprios**
Conferir fluxo integrado no binário final com riscos das mudanças, sem repetir casos inalterados por rotina. Atualizar matriz desktop-native/docs/chat-parity.md com implementado/conferido/pendente para chat, compositor e barra direita. Manter demo ligada ao backend real em unidade independente, fechar fixtures/cx/agentes concluídos e monitor de quota ao terminar/transferir. Índices vazios, checkout principal preservado, sem commit/push/testes executados sem pedido. Windows/macOS e desempenho seguem sem alegações sem prova.

### Task 6: Paridade visual do layout e prévia duplicada
Status: ready-for-agent
Risk: high
Origem: ordem do usuário em 2026-09-24 após ver o binário final: "é um port pro Rust, então basicamente vou querer que sejam parecidos"; "aquilo que for melhor no Rust até pode ficar, mas não pode deixar coisas como o composer está e achar que isso é identidade do Rust — isso foi desleixo, está tudo jogado". Substitui, para compositor/sidebar/barra direita, a régua anterior "manter visual nativo aprovado".
Régua visual (bar): destino de layout é o app web/Electron atual — `frontend/src/components/Composer.svelte`, `Sidebar.svelte`, `DesktopSessionContext.svelte` (e bolhas/ferramentas de `Chat.svelte` onde o nativo destoar). Mesma organização (o que fica onde, agrupamento, hierarquia, ícones, espaçamento, estados). Diferença só onde o Rust fica melhor ou o GPUI não permite, declarada item a item com o motivo; nada solto ou "jogado". Tema/vidro do Hangar mantidos, sem vazar conteúdo de trás a ponto de competir com o texto.
Referência técnica: Zeron (https://github.com/zeronsh/zeron), cliente nativo em GPUI — consultar como ele resolve composer, painéis, ícones e layout quando travar; não copiar engine, autenticação ou sincronização.
Arquivos: módulos de apresentação Rust (app.rs, app/side.rs, app/controls.rs, theme.rs, novos se precisar), assets de ícone, traduções native_*, docs.
- [x] **Step 16: Corrigir a prévia duplicada com prova real**
Reproduzir numa sessão real trabalhando (só leitura: abrir/assistir, sem enviar) ou em cx própria: a resposta aparece gravada e de novo como prévia com "Trabalhando". Achar a causa (prévia não descartada quando o bloco real chega) e corrigir; provar no binário final com captura antes/depois e numa sessão real.
- [x] **Step 17: Portar o layout do compositor, sidebar e barra direita — verificação manual**
Capturar a referência web/Electron de cada área em cada estado (compositor vazio, com texto, com anexos, com seletor aberto, trabalhando; sidebar com estados/seleção; barra direita com e sem dados) e reproduzir no nativo. Seletores de modelo/esforço/modo dentro do compositor como no web, botões com ícone, dicas no lugar do web.
- [x] **Step 18: Conferir lado a lado e entregar**
Folha lado a lado web × nativo por área e estado, mesma sessão/estado; lista de diferenças declaradas com motivo. Funcionalidade das Tasks 4–5 sem regressão nos caminhos tocados. Demo final atualizada (unidade hangar-native-final-view).

### Task 7: Rolagem suave da conversa
Status: ready-for-agent (começa depois do fechamento da Task 6; um escritor por vez)
Risk: high
Origem: pedido direto do usuário em 2026-09-24 no terminal do executor da Task 6: a rolagem da conversa não está suave; no web é suave.
Diagnóstico do executor da Task 6 (não verificado pela árbitra): o nativo usa `FollowMode::Tail`, que salta a cada commit, e uma linha de lista por mensagem. O Zeron (`crates/ui/src/transcript.rs`) usa mola de velocidade (StickSpring) que desliza até o fim quando chega texto, uma linha por bloco de Markdown e cache de linhas por impressão digital.
- [x] **Step 19: Medir e reproduzir o salto**
Reproduzir em sessão real só assistindo (ou cx própria) e registrar o comportamento atual frente ao web (vídeo curto ou sequência de capturas), incluindo acompanhar o fim durante streaming, roda do mouse e voltar ao fim.
- [x] **Step 20: Rolagem suave — verificação manual**
Deslizar até o fim com o texto novo em vez de saltar; preservar a leitura quando a pessoa rolou para cima (não puxar para o fim); rolagem da roda suave. Consultar o Zeron para a mola e a granularidade; não copiar engine. Sem regressão no streaming (Task 1) e na lista janelada.

### Task 8: Memória de imagem com limite
Status: ready-for-agent (depois da Task 7; um escritor por vez)
Risk: high
Origem: levantamento do Zeron pedido pelo usuário (`~/.hangar/orq/2026-09-23-native-parity/zeron-survey.md`, item 1). Defeito, não melhoria: `media` em app.rs nunca é limpo e a imagem inteira fica decodificada para ser mostrada em 320×240; a memória cresce sem limite numa sessão longa com imagens.
- [x] **Step 21: Medir e limitar a memória de imagem**
Medir o crescimento (RSS antes/depois de abrir uma conversa com muitas imagens e trocar de sessão). Miniatura com tamanho limitado, cache com teto (referência: LRU de 64 MB do Zeron, attachments.rs), liberar o que sai do cache sem apagar as visíveis; com gpui-kit 0.6.6 conferir `cx.drop_image`/`remove_asset` antes de usar. Prova: medição antes/depois e imagens ainda nítidas na tela.

### Task 12: Desenho inspirado no Zeron + configurações no nativo
Status: ready-for-agent (depois da Task 8 e antes da Task 9, por ordem do usuário)
Risk: high
Origem: usuário em 2026-09-24 depois de ver o Zeron rodando: "não é uma cópia, é uma inspiração neles, mas com as nossas também"; "a sidebar deles o padrão pode ser a deles, mas no nosso já tem a opção de ser solta e não colada, e aí fica a nossa, isso fica nas opções de aparência"; "o nosso já tem um visual e uma direção que quero seguir, mas o deles tem coisas melhores e mais profissionais"; "a tela de configs deles com certeza é melhor que a minha… mas tem que ter as minhas opções; ele entra só como referência de organização e de layout padrão". Escolha A: só no app nativo; o web fica como está.
Régua: o desenho do Zeron vira o PADRÃO do nativo (organização do chat, sidebar colada, configurações com navegação por seção e cartões com ícone/título/descrição/controle à direita); o visual atual do Hangar (sidebar solta, vidro, paleta) continua como OPÇÃO de Aparência, com melhorias. Nenhuma função do Hangar sai. Configurações como PÁGINA igual à do Zeron (a barra lateral vira a navegação das seções, com Voltar; conteúdo no centro), por ordem do usuário em 2026-09-24 ~12:40: "não disse pra fazer igual o do Zeron só que com as minhas opções?". A regra "config em modal" do CLAUDE.md segue valendo para o web. Fonte das opções: o modal de configurações do web (`frontend/src/…` Settings), só leitura. Referência: Zeron rodando local (unidade zeron-view, pasta pessoal temporária) e seu código; inspiração, não cópia de código.
- [ ] **Step 28: Folha de desenho para aprovação do usuário (sem código de produto)**
Capturas lado a lado: Zeron × nativo atual × proposta (mock) para chat, sidebar colada/solta, compositor e modal de configurações (Aparência e uma seção de servidor). Inventário das seções/opções do modal web e onde cada uma fica. O usuário aprova ou corrige antes de qualquer código; a aprovação vira o detalhamento dos Steps seguintes.
- [ ] **Step 29: Implementar o desenho aprovado — verificação manual**
Recorte e ordem definidos na aprovação do Step 28 (pode virar mais de uma Task se o inventário for grande). Prova lado a lado com a folha aprovada e com o Zeron; alternância padrão Zeron × visual Hangar nas opções de Aparência sem perder função.

### Task 9: Popover, cópia de código, notificação e fontes
Status: ready-for-agent (depois da Task 12)
Risk: high
Origem: escolha B do usuário em 2026-09-24 sobre o levantamento do Zeron (`zeron-survey.md`, itens 5, 6, 7, 8). Referências de arquivo:linha estão no levantamento; conferir contra gpui-kit 0.6.6.
- [ ] **Step 22: Popover preso ao gatilho e fechando ao clicar fora**
Seletores e painéis do compositor ancorados no botão que os abriu, sem vazar clique para o que está atrás, fechando ao clicar fora (Zeron popover.rs: `deferred(anchored())` + `occlude` + `on_mouse_down_out`).
- [ ] **Step 23: Copiar bloco de código, notificação do sistema e fontes embutidas — verificação manual**
GIF animado na prévia da conversa como no web (task8-r1 NOTED 1: `img` precisa de `.id()` e redesenho contínuo só enquanto o GIF está visível, dentro do teto de memória da Task 8); botão copiar em bloco de código (gpui-kit `TextView::code_block_actions` se existir na 0.6.6); notificação do sistema quando a sessão selecionada termina ou pede resposta com a janela sem foco, no mesmo critério do web; fontes da interface embutidas com fallback, sem depender da JetBrainsMono instalada.

### Task 10: Markdown estável durante o streaming
Status: ready-for-agent (depois da Task 9)
Risk: high
Origem: escolha B do usuário (`zeron-survey.md`, itens 3 e 4).
- [ ] **Step 24: Fechar marcação pendente só na exibição e esmaecer o texto novo**
O texto em streaming não reflui quando fecha `**`, `` ` `` ou `[link](` (Zeron mend.rs `close_hanging`, aplicado só na exibição); texto novo entra esmaecendo (gpui-kit `TextViewMotion::with_stream_fade`, se existir na 0.6.6). O transcript gravado não muda.
- [ ] **Step 25: Anel de contexto correto sobre o vidro — verificação manual**
Contorno para caminho com alfa somado em janela transparente (fundo opaco atrás do anel ou desenho com quads/SVG), sem mudar o vidro do resto da janela.

### Task 11: @menção de arquivo no compositor
Status: ready-for-agent (depois da Task 10)
Risk: high
Origem: escolha B do usuário (`zeron-survey.md`, item 9).
- [ ] **Step 26: Confirmar a rota de busca de arquivos que o backend já tem**
Backend não muda. Se não existir rota de busca/lista de arquivos da sessão utilizável, a Task para e volta à árbitra com o que falta, sem simular.
- [ ] **Step 27: @menção com lista, busca e inserção do caminho — verificação manual**
Digitar `@` abre lista filtrada dos arquivos do projeto da sessão, teclado e mouse, insere o caminho no rascunho como o web faz; nada é enviado sozinho.

## Cotas e sucessão
Config dirs conferidos: Jefferson=/home/jefferson/.claude-jefferson;200-3=/home/jefferson/.claude-claude-200-3;200-5=/home/jefferson/.claude-claude-200-5. Não deduzir caminho pelo apelido. Helper/monitor e regras atuais em /home/jefferson/.hangar/orq/2026-09-23-native-parity/succession.md. Dados de cota precisam ser relidos antes de alocar; nunca consumir reset credit automaticamente. Ao atingir50% Codex pessoal, transferir coordenação e cessar essa conta. Histórico de escolhas anteriores está em registro.md, não autoriza alocações antigas.

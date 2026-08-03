# Demo do claude-cockpit

Storyboard público para as capturas de tela e o vídeo de demonstração. Esta página descreve o que
será mostrado sem depender de uma conta, sessão ou custo reais.

> **Dados de demonstração:** todas as mensagens, nomes de sessões, perguntas, modelos, valores,
> estados e imagens usados nas capturas e no vídeo são fictícios. Os valores da tela de custos são
> sintéticos e não representam uma cobrança, fatura, conta ou consumo real.

## Storyboard

### 1. Chat no celular — acompanhar uma resposta

- **Objetivo:** mostrar o fluxo principal no telefone, do envio à resposta em markdown.
- **Ação:** abrir uma sessão de demonstração no chat mobile e enviar uma solicitação curta, como
  `Resuma em três pontos o plano de lançamento.`
- **Captura:** o prompt enviado, o indicador de trabalho/preview e a resposta final legível.
- **Mensagem para o leitor:** o app acompanha uma sessão viva sem exigir que a pessoa fique diante
  do terminal.
- **Dados:** sessão `demo-mobile`, prompt e resposta inventados.

### 2. Ask question — escolher uma opção

- **Objetivo:** mostrar que uma pergunta interativa pode ser respondida pelo celular.
- **Ação:** exibir a pergunta fictícia `Qual estilo de demo você prefere?` com as opções
  `Conciso` e `Detalhado`; tocar em `Conciso`.
- **Captura:** o stepper de pergunta nativo antes da escolha e a confirmação da opção escolhida.
- **Mensagem para o leitor:** opções interativas aparecem como controles tocáveis, em vez de exigir
  a navegação no TUI.
- **Dados:** pergunta, opções e resposta inventadas; nenhuma escolha real deve aparecer.

### 3. Board no desktop — acompanhar sessões diferentes

- **Objetivo:** mostrar a visão agregada das sessões no desktop.
- **Ação:** abrir o board com três cards fictícios: uma sessão **Claude** trabalhando, uma
  sessão **Pi** pronta e uma sessão **Codex** pronta.
- **Captura:** as três colunas por estado, com os nomes Claude, Pi e Codex claramente visíveis e
  mini-conversas curtas nos cards.
- **Mensagem para o leitor:** uma única visão permite identificar rapidamente qual sessão precisa de
  atenção e qual continua trabalhando.
- **Dados:** estados, nomes, textos e status line sintéticos.

### 4. Canvas no desktop — organizar tiles

- **Objetivo:** mostrar a organização livre das sessões.
- **Ação:** abrir o canvas e posicionar tiles fictícios de sessões Claude, Pi e Codex em uma
  composição por assunto; ajustar o tamanho de um tile e deixar outro separado.
- **Captura:** tiles flutuantes em posições e dimensões diferentes, com alças/identificação
  suficientes para entender que podem ser reorganizados.
- **Mensagem para o leitor:** o canvas é uma alternativa ao agrupamento em colunas para quem prefere
  organizar o espaço manualmente.
- **Dados:** posições, tamanhos, nomes e conversas inventados.

### 5. Pi model picker — alternar provedores e modelos

- **Objetivo:** mostrar a escolha de modelo específica de uma sessão Pi.
- **Ação:** abrir o seletor de modelo e mostrar duas sessões fictícias: uma usando uma conta
  **GPT/OpenAI** e outra usando um modelo gratuito do **OpenRouter**; selecionar um modelo em cada
  sessão, sem exibir credenciais.
- **Captura:** o catálogo e o nível de pensamento/effort selecionado, com os nomes dos provedores e
  modelos demonstrativos visíveis nas capturas GPT/OpenAI e OpenRouter free.
- **Mensagem para o leitor:** cada sessão Pi pode escolher seu modelo e nível de raciocínio sem
  depender de uma lista fixa no app.
- **Dados:** contas, ids, modelos e seleções fictícios; não usar chaves, tokens ou endpoints
  privados.

### 6. Claude engine — configurar um motor alternativo

- **Objetivo:** mostrar o cadastro de um provedor alternativo para uma sessão Claude.
- **Ação:** abrir a configuração de motores, preencher um nome demonstrativo e um endpoint público
  de exemplo; manter o campo de chave mascarado/oculto e não salvar uma chave real na captura.
- **Captura:** o formulário com nome, endpoint e modelo de exemplo, além do estado de chave
  mascarada (por exemplo, `sk-••••1234`), sem qualquer valor copiável.
- **Mensagem para o leitor:** um motor alternativo muda o provedor da sessão sem expor a chave nem
  misturar a configuração com a conta principal.
- **Dados:** nome, endpoint e modelo fictícios. A demonstração não deve testar nem persistir uma
  credencial real.

### 7. Settings — encontrar as configurações

- **Objetivo:** apresentar a organização das configurações sem transformar a demo em um tutorial de
  credenciais.
- **Ação:** abrir o modal de configurações e percorrer brevemente os grupos/seções disponíveis,
  destacando onde ficam aparência, configurações gerais e integrações.
- **Captura:** a estrutura dos grupos, títulos e controles neutros; não preencher campos de segredo,
  token, chave, URL privada ou caminho pessoal.
- **Mensagem para o leitor:** as configurações ficam agrupadas e podem ser consultadas sem mostrar
  dados do ambiente do usuário.
- **Dados:** rótulos e valores de exemplo não sensíveis.

### 8. Costs — dados de demonstração

- **Objetivo:** mostrar a leitura do painel de custos sem sugerir cobrança real.
- **Ação:** abrir `Custos` com o período de demonstração e dados sintéticos de fontes Claude, Pi e
  Codex; manter o rótulo **`Demo data`** visível durante toda a captura.
- **Captura:** KPIs, gráfico por dia, rankings e filtros com números fictícios; o aviso de demo deve
  estar legível e não pode ser coberto por outro elemento.
- **Mensagem para o leitor:** o painel explica volume e estimativa sintética de uso; não é uma fatura.
- **Dados:** valores, datas, projetos, provedores e modelos inventados. Não capturar o painel com
  custos ou consumo da conta real.

### 9. Vídeo — percurso de 30–45 segundos

- **Objetivo:** condensar o fluxo de maior valor em uma apresentação curta.
- **Sequência sugerida:**
  1. **0–8 s — cena 1:** no celular, mostrar o prompt, a resposta e o acompanhamento da sessão.
  2. **8–18 s — cena 3:** cortar para o desktop board e mostrar Claude, Pi e Codex em estados
     diferentes.
  3. **18–28 s — cena 5:** mostrar o chat Pi com a identificação do runtime.
  4. **28–32 s — cena 5:** mostrar o seletor de modelos e os níveis de raciocínio do Pi, sem
     mostrar chaves.
  5. **32–40 s — cena 8:** encerrar no painel de custos com `Demo data` e rankings sintéticos.
- **Captura:** gravação contínua ou cortes com transições simples; os cinco momentos devem ser
  compreensíveis mesmo sem áudio.
- **Contrato:** o vídeo usa exclusivamente dados fictícios, não inclui Git como destaque e não
  mostra credenciais, custos reais ou identificadores pessoais.

## Lista de capturas

| ID | Arquivo sugerido | Cena | Conteúdo obrigatório |
| --- | --- | --- | --- |
| `mobile-chat` | `docs/img/mobile-chat-demo.png` | 1 | prompt, resposta e estado de acompanhamento |
| `ask-question` | `docs/img/mobile-ask-question.png` | 2 | pergunta fictícia e opções tocáveis |
| `desktop-board` | `docs/img/desktop-board-demo.png` | 3 | Claude, Pi e Codex em estados distintos |
| `desktop-canvas` | `docs/img/desktop-canvas-demo.png` | 4 | tiles reorganizados e com tamanhos variados |
| `pi-model-picker` | `docs/img/desktop-models-demo.png`, `docs/img/desktop-openrouter-free-demo.png` | 5 | GPT/OpenAI, OpenRouter free e nível de raciocínio |
| `claude-engine` | `docs/img/desktop-engines-demo.png` | 6 | motor alternativo e chave mascarada |
| `settings` | `docs/img/desktop-settings-demo.png` | 7 | grupos de configurações sem valores sensíveis |
| `costs-demo` | `docs/img/desktop-costs-demo.png` | 8 | `Demo data` e valores sintéticos |
| `demo-video` | `docs/demo/claude-cockpit-overview.webm` | 9 | cenas de chat, board, Pi, modelos e custos em 40 s |

Os arquivos acima são os artefatos públicos atuais. Eles foram revisados para conter apenas dados
sintéticos; o vídeo é uma montagem curta das capturas aprovadas.

## Contrato de sanitização

Antes de guardar ou publicar uma captura, revisar a imagem e a gravação inteira — inclusive barras
de navegador, notificações, tooltips, rodapé, painel lateral e quadros intermediários — com este
checklist:

- usar somente sessões, prompts, respostas, nomes, caminhos, datas e estados inventados;
- manter `Demo data` visível na cena de custos e usar números sintéticos;
- não mostrar chaves de API, tokens, cookies, QR codes de login, URLs privadas ou endpoints de rede
  internos;
- não mostrar e-mails, nomes de usuário, nomes de máquina, nomes de organização, ids de sessão,
  hashes, caminhos pessoais ou conteúdo de outros projetos;
- no motor Claude, deixar a chave vazia ou mascarada; nunca usar uma chave real apenas para
  produzir a captura;
- no Pi model picker, usar apenas os rótulos demonstrativos GPT/OpenAI e OpenRouter gratuito,
  sem revelar conta, assinatura ou credencial real;
- não incluir Git como cena ou destaque visual; qualquer indicação incidental que não seja necessária
  deve ser removida ou coberta antes da publicação;
- confirmar que a imagem e o vídeo não permitem recuperar um segredo por seleção de texto, zoom,
  quadro pausado, metadado ou faixa de áudio;
- remover metadados que revelem o ambiente antes de publicar; se houver dúvida, descartar a captura
  e refazê-la com um perfil limpo.

A revisão final deve confirmar que cada arquivo da lista contém apenas o conteúdo obrigatório da
cena correspondente e que a declaração de dados fictícios continua válida para o conjunto inteiro.

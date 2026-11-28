// Rótulos em português dos eventos, dimensões e preferências que a telemetria manda.
// Valor que não está aqui aparece como veio.
export type IconName = "search" | "send" | "reply" | "square" | "link" | "list" | "bug" | "pencil" | "key" | "x" | "globe" | "users" | "refresh"
  | "terminal" | "plus" | "external" | "filePen" | "file" | "check" | "chat" | "wrench" | "zap";

// [título, grupo, o que mede, em uma frase].
export const EVENTS: Record<string, [title: string, group: string, explain: string]> = {
  app_visible_minutes: ["Minutos com o app aberto", "uso", "Tempo com o app do Hangar na tela, no celular ou no PC."],
  message_sent: ["Mensagens enviadas", "uso", "Mensagens que as pessoas mandaram aos agentes pelo app ou pela API."],
  turn_finished: ["Respostas do agente", "uso", "Cada vez que o agente trabalhou numa mensagem, do começo até parar."],
  session_opened: ["Sessões abertas", "uso", "Sessões novas, com o tempo do pedido até a sessão abrir."],
  feature_used: ["Recursos usados", "uso", "Ações que deram certo: responder pergunta, renomear, buscar, parear…"],
  mcp_tool: ["Ferramentas usadas pelos agentes", "uso", "Chamadas das ferramentas do Hangar (navegador, página, recado) feitas por um agente."],
  voice_call: ["Chamadas de voz", "uso", "Conversas por voz com o agente, com a duração."],
  dictation: ["Ditado", "uso", "Textos ditados, do fim da fala até o texto ficar pronto."],
  chat_open: ["Abrir uma conversa", "uso", "Do toque numa conversa até ela aparecer na tela."],
  client_route: ["Caminho até o servidor", "uso", "Por onde o app chegou ao servidor (rede local, Tailscale, Connect) e quanto levou para achar."],
  peers: ["Máquinas ligadas entre si", "uso", "Quantas outras máquinas cada servidor conhece, por tipo de ligação."],
  problem_shown: ["Avisos de problema na tela", "saude", "Avisos que apareceram na lista de sessões para a pessoa."],
  problem_hidden: ["Problemas só no diário", "saude", "Problemas que o Hangar registrou sem mostrar na tela, por serem só de diagnóstico."],
  session_open_failed: ["Sessões que não abriram", "saude", "Pedidos de sessão nova que falharam, com o motivo."],
  send_failed: ["Mensagens recusadas", "saude", "Mensagens que o servidor não conseguiu entregar ao agente."],
  session_fallen: ["Sessões que caíram", "saude", "Sessões com terminal que morreram sozinhas (falta de memória ou terminal fechado)."],
  server_exit: ["Como o servidor parou", "saude", "Contado na subida seguinte: queda, atualização ou parada pedida."],
  limit_switch: ["Troca de conta no limite", "saude", "Quando uma conta bate o limite de uso e o Hangar tenta outra."],
  client_fatal: ["Quedas do app", "saude", "O app do celular ou do PC fechou por erro."],
  update_finished: ["Atualizações", "saude", "Fim de cada atualização: de qual versão para qual, e se deu certo."],
  update_step: ["Etapas da atualização", "saude", "Cada etapa da atualização: baixar, trocar a versão, rodar os passos e reiniciar."],
  update_failed_code: ["Falhas de atualização", "saude", "O código do erro de cada atualização que falhou."],
  server_startup: ["Subida do servidor", "saude", "Do início do servidor até ele aceitar pedidos, depois de ligar, reiniciar ou atualizar."],
  log_dropped: ["Linhas de log que sobraram", "saude", "Linhas novas de aviso ou erro que não couberam no teto de 100 por dia."],
  diary_dropped: ["Itens do diário que não couberam", "saude", "Registros de falha e passos do caminho cortados na máquina antes do envio do pacote."],
};
// Preferências do retrato diário (evento settings): [título, grupo, o que é].
export const SETTING_GROUPS = ["Aparência", "Conversa", "Barra e navegação", "Beta", "Servidor"];
export const SETTINGS: Record<string, [title: string, group: string, explain: string]> = {
  theme: ["Tema", "Aparência", "Claro, escuro, automático (segue o sistema) ou as cores do papel de parede."],
  panels: ["Painéis", "Aparência", "Caixas coladas na janela ou soltas sobre o fundo (PC)."],
  palette: ["Paleta", "Aparência", "Cores clássicas do Hangar ou neutras."],
  background: ["Fundo", "Aparência", "O que fica atrás das caixas."],
  background_effect: ["Efeito do fundo", "Aparência", "Efeito desenhado por cima da imagem de fundo."],
  wallpaper: ["Papel de parede", "Aparência", "Com o fundo da área de trabalho: ver através da janela ou desenhar a foto dentro (PC)."],
  surface_material: ["Material das caixas", "Aparência", "Vidro que deixa ver o fundo ou caixa opaca (PC)."],
  reading: ["Leitura", "Aparência", "O que segura o texto da conversa sobre o fundo."],
  font: ["Fonte da conversa", "Aparência", "Fonte do sistema, monoespaçada ou outra instalada (o nome dela não sai)."],
  code_font: ["Fonte do código", "Aparência", "Fonte dos blocos de código (PC)."],
  text_size: ["Tamanho do texto", "Aparência", "Texto da conversa menor, igual ou maior que o de fábrica."],
  compact_style: ["Estilo compacto", "Aparência", "Se a aparência é exatamente a do atalho Estilo compacto (PC)."],
  tool_look: ["Estilo da conversa", "Conversa", "Como as ferramentas do agente aparecem na conversa."],
  thinking_tools: ["Ferramentas no pensamento", "Conversa", "Quais chamadas feitas no meio do raciocínio ficam dentro do bloco do pensamento."],
  thinking_collapsed: ["Pensamento dobrado", "Conversa", "Pensamento terminado numa linha só, aberto no clique (PC)."],
  task_list: ["Lista de tarefas", "Conversa", "Tarefas do agente como bloco de progresso na conversa."],
  table_chart: ["Gráfico nas tabelas", "Conversa", "Botão de gráfico sobre tabelas numéricas."],
  ask_highlight: ["Destaque da pergunta", "Conversa", "Cor da moldura quando o agente pergunta."],
  delivery: ["Mensagem com o agente trabalhando", "Conversa", "O que acontece com a mensagem mandada enquanto a sessão trabalha."],
  hands_free: ["Mãos livres", "Conversa", "Ditado que envia sozinho."],
  navigation: ["Navegação", "Barra e navegação", "Onde ficam as sessões no PC."],
  sidebar_compact: ["Barra compacta", "Barra e navegação", "Linhas mais baixas na barra de sessões (PC)."],
  sidebar_height: ["Altura da barra", "Barra e navegação", "Barra da altura da janela ou do conteúdo (PC)."],
  sidebar_group: ["Agrupar a barra", "Barra e navegação", "Como a barra do PC agrupa as sessões."],
  tabs_full: ["Abas detalhadas", "Barra e navegação", "Abas com pasta e branch embaixo do nome (PC)."],
  list_group: ["Agrupar a lista", "Barra e navegação", "Como a lista do celular agrupa as sessões."],
  list_organize: ["Organizar a lista", "Barra e navegação", "Lista do celular por sessões ou por conversas."],
  home_screen: ["Tela inicial", "Barra e navegação", "O que o celular abre primeiro."],
  tablet_bar: ["Barra no tablet", "Barra e navegação", "Onde fica a barra de sessões no tablet."],
  language: ["Idioma", "Barra e navegação", "Idioma do app."],
  tips: ["Dicas", "Barra e navegação", "Dicas do app ligadas ou desligadas (PC)."],
  tour: ["Tour", "Barra e navegação", "Se a pessoa terminou, pulou ou ainda não viu o tour (PC)."],
  conversation_beta: ["Conversa nova", "Beta", "O desenho novo da conversa, da tela Beta."],
  code_mode: ["Code Mode do Claude Code", "Beta", "Sessões do Claude abertas com o Code Mode."],
  default_provider: ["Agente padrão", "Servidor", "O agente que abre quando o pedido não diz qual."],
  default_effort: ["Esforço padrão", "Servidor", "O esforço salvo como padrão para o agente padrão."],
  auto_report: ["Relatório automático", "Servidor", "Se a atualização que falha manda o relatório sozinha."],
  dictation_org: ["Organizar o ditado", "Servidor", "Quem organiza o texto ditado depois da transcrição."],
  dictation_provider: ["Transcrição", "Servidor", "O serviço que transforma a fala em texto."],
  connect: ["Hangar Connect", "Servidor", "Máquina ligada ao Connect."],
  computer_use: ["Computer Use", "Servidor", "Controle do desktop ligado para os agentes."],
  whatsapp: ["WhatsApp", "Servidor", "Integração do WhatsApp instalada."],
  jev: ["Jev", "Servidor", "Chave do Jev configurada."],
};
export const SETTING_VALUES: Record<string, string> = { on: "ligado", off: "desligado", auto: "automático", light: "claro", dark: "escuro", desktop: "área de trabalho",
  attached: "colados", floating: "soltos", classic: "clássico", neutral: "neutra", plain: "liso", texture: "textura", aurora: "luz no canto",
  image: "imagem", none: "nenhum", dither: "pontilhado", ascii: "ASCII", halftone: "retícula", scanlines: "linhas de TV", window: "janela",
  glass: "vidro", opaque: "opaco", text: "texto", sheet: "folha", system: "do sistema", mono: "monoespaçada", custom: "outra instalada",
  jetbrains_mono: "JetBrains Mono", small: "menor", normal: "de fábrica", large: "maior", chips: "chips", tree: "árvore", terminal: "terminal",
  claude_desktop: "Claude Desktop", search: "só buscas", all: "todas", accent: "cor de destaque", amber: "âmbar", queue: "fila",
  direct: "direto", jev: "Jev escolhe", conversation: "na conversa", tabs: "abas em cima", bottom_tabs: "abas embaixo",
  conversations: "conversas", sidebar: "barra lateral", full: "inteira", content: "do conteúdo", project: "projeto", machine: "máquina",
  server: "servidor", sessions: "sessões", new_conversation: "conversa nova", left: "à esquerda", bottom: "embaixo", top: "em cima",
  pt: "português", en: "inglês", done: "terminou", skipped: "pulou", unset: "sem padrão", minimal: "mínimo", low: "baixo", medium: "médio",
  high: "alto", xhigh: "muito alto", max: "máximo", ultra: "ultra", unanswered: "sem resposta", harness: "pelo agente",
  external_api: "por API externa", openai: "OpenAI (ou compatível)", groq: "Groq", elevenlabs: "ElevenLabs", whisper_cpp: "whisper.cpp local",
  installed: "instalado", no: "não", configured: "configurado", other: "outro" };
export const AGENTS: Record<string, string> = { claude: "Claude", codex: "Codex", pi: "Pi", omp: "omp", kimi: "Kimi", antigravity: "Antigravity" };
// Valores das dimensões em português; o que não está aqui aparece como veio.
export const VALUES: Record<string, Record<string, string>> = {
  provider: { claude: "Claude", codex: "Codex", pi: "Pi", omp: "omp", kimi: "Kimi", antigravity: "Antigravity", other: "outro agente" },
  surface: { terminal: "com terminal", headless: "sem terminal" },
  origin: { person: "pela pessoa", session: "por outra sessão", manual: "pelo botão", auto: "automática" },
  client: { ios: "iPhone/iPad", android: "Android", pwa: "navegador", native: "app do PC", server: "servidor", other: "outro app" },
  kind: { lan: "rede local", tailscale: "Tailscale", connect: "Hangar Connect", public: "endereço público", disabled: "desligada",
    crash: "queda", update: "atualização", requested: "parada pedida" },
  cause: { package: "troca de versão", terminal_out_of_memory: "sem memória", terminal_died: "terminal fechou" },
  result: { updated: "atualizou", updated_manual: "atualizou (à mão)", up_to_date: "já estava em dia", failed: "falhou" },
  step: { download: "baixar", swap: "trocar a versão", steps: "rodar os passos", restart: "reiniciar", other: "outra" },
  route: { principal: "endereço principal", rede_local: "rede local", tailscale: "Tailscale", connect: "Hangar Connect",
    ditado: "ditado", relimpar: "refazer limpeza", sessao: "na sessão" },
  channel: { latest: "estável", beta: "testes", other: "outro" },
  status: { ok: "deu certo", warning: "com aviso", error: "com erro", cancelled: "cancelado" },
  effort: { low: "esforço baixo", medium: "esforço médio", high: "esforço alto", xhigh: "esforço muito alto", max: "esforço máximo" },
  permission: { default: "pede permissão", acceptEdits: "aceita edições", plan: "modo plano", bypassPermissions: "sem pedir permissão",
    bypass_permissions: "sem pedir permissão", auto: "automático", "read-only": "só leitura", "workspace-write": "escreve na pasta",
    "danger-full-access": "acesso total" },
  feature: { search: "busca", ask_history: "perguntar ao histórico", broadcast: "mandar para várias", tags: "etiquetas",
    "session.answer": "responder pergunta", "session.select": "escolher opção", "session.question_skip": "pular pergunta",
    "session.interrupt": "interromper", "session.task_stop": "parar tarefa", "session.btw": "comentário de lado",
    "session.rename": "renomear sessão", "session.reopen": "reabrir sessão", "session.dismiss": "dispensar sessão",
    "session.close": "fechar sessão", "session.resume": "retomar sessão", "session.reload": "recarregar sessão",
    "session.surface": "trocar terminal/sem terminal", "session.account": "trocar conta", "session.model": "trocar modelo",
    "session.provider": "trocar agente", "session.permission_mode": "trocar permissão", "session.codex_mode": "modo do Codex",
    "session.plan_implement": "implementar plano", "session.service_tier": "modo rápido", "session.notice_fix": "corrigir aviso",
    "session.queue": "mexer na fila", "session.hide": "esconder sessão", "session.shell": "abrir shell",
    "session.open_editor": "abrir no editor", "session.open_terminal": "abrir terminal", "session.browser": "navegador da sessão",
    "session.loop": "repetir (loop)", "session.then": "depois disso", "session.subagent_message": "falar com subagente",
    "session.port_stop": "parar porta", "session.pair": "parear sessões", "session.pair_external": "parear com outra pessoa",
    "session.group_message": "recado ao grupo", "session.share": "compartilhar sessão", "archive.resume": "retomar do arquivo",
    "plugins.install": "instalar plugin", "vault.add": "guardar senha", "vault.use": "usar senha do cofre",
    keyring_unlock: "destrancar chaveiro", diag_report: "reportar problema", device_install: "instalar no aparelho",
    config_sync: "sincronizar configuração", "push.mute": "silenciar notificações" },
};
export const KEYS: Record<string, string> = { from: "de", to: "para", tool: "ferramenta", code: "código", model: "modelo", effort: "esforço", permission: "permissão", status: "resultado" };
// Sim/não que só vale dizer quando é "sim" (ou quando "não" é a notícia).
export const FLAGS: Record<string, [yes: string | null, no: string | null]> = { steer: ["corrigindo o rumo", null], restore: ["restaurada", null], jev: ["com Jev", null], prewarm: ["pré-aquecida", null],
  worktree: ["em worktree", null], failed: ["falhou", null], error: ["com erro", "sem erro"], ok: ["deu certo", "falhou"] };
// Eventos que são falha de verdade: vão à seção Erros, em vermelho.
export const BAD = new Set<string>(["problem_shown", "session_open_failed", "send_failed", "session_fallen", "client_fatal", "update_failed_code"]);
export const BUCKETS = ["lt100ms", "lt300ms", "lt1s", "lt3s", "lt10s", "lt30s", "ge30s"];
export const BUCKET_LABEL: Record<string, string> = { lt100ms: "menos de 100 ms", lt300ms: "100–300 ms", lt1s: "0,3–1 s", lt3s: "1–3 s", lt10s: "3–10 s", lt30s: "10–30 s", ge30s: "30 s ou mais" };

// Ícones Lucide dos recursos e das ferramentas; o que não está aqui usa o genérico.
export const FEATURE_ICON: Record<string, IconName> = { search: "search", ask_history: "search", broadcast: "send", "session.answer": "reply", "session.select": "reply",
  "session.interrupt": "square", "session.task_stop": "square", "session.pair": "link", "session.pair_external": "link", "session.queue": "list",
  diag_report: "bug", "session.rename": "pencil", "vault.use": "key", "vault.add": "key", keyring_unlock: "key", "session.close": "x",
  "session.browser": "globe", "session.group_message": "send", "session.share": "users", "session.reopen": "refresh", "session.resume": "refresh",
  "archive.resume": "refresh", "session.reload": "refresh", "session.shell": "terminal", "session.open_terminal": "terminal", "plugins.install": "plus" };
export const TOOLS: Record<string, [label: string, icon: IconName]> = { browser: ["Navegador", "globe"], browser_open: ["Abrir navegador", "external"], browser_batch: ["Navegador em lote", "globe"],
  browser_goal: ["Objetivo no navegador", "globe"], browser_check: ["Conferir no navegador", "globe"], send: ["Recado entre sessões", "send"],
  group: ["Aviso ao grupo", "send"], sessions: ["Ver sessões", "list"], who_am_i: ["Quem sou eu", "users"], pair: ["Parear sessões", "link"],
  unpair: ["Desfazer par", "link"], contract: ["Contrato do grupo", "filePen"], new_session: ["Nova sessão", "plus"],
  close_session: ["Fechar sessão", "x"], interrupt: ["Interromper filha", "square"], html_render: ["Página na conversa", "file"],
  request_secret: ["Pedir senha", "key"], vault_list: ["Listar o cofre", "key"], vault_fill: ["Preencher senha", "key"],
  vault_exec: ["Comando com senha", "key"], vault_file: ["Arquivo com senha", "key"], mission: ["Etapa do ticket", "check"],
  whatsapp_chats: ["WhatsApp: conversas", "chat"], whatsapp_read: ["WhatsApp: ler", "chat"], whatsapp_search: ["WhatsApp: buscar", "chat"],
  whatsapp_request_media: ["WhatsApp: mídia", "chat"], whatsapp_draft: ["WhatsApp: rascunho", "chat"], other: ["outra ferramenta", "wrench"] };

// Esforço do mais leve ao maior, numa rampa de anil; valor fora da lista fica cinza.
export const EFFORTS: [string, string][] = [["minimal", "#e4e6fc"], ["low", "#c9cdf8"], ["medium", "#a8aff3"], ["high", "#8b93f0"], ["xhigh", "#6670d9"], ["max", "#4a52b8"], ["ultra", "#353b8f"]];
export const MODEL_LABEL: Record<string, string> = { engine: "motor próprio", other: "outro modelo", "": "sem modelo" };

// A tela e a cópia mostram só a projeção conhecida, mesmo se o receptor guardar outros campos.
export const FIELD_LABEL: Record<string, string> = { action: "Ação", codigo: "Código", etapa: "Etapa", erro_tipo: "Tipo do erro", ms: "Duração (ms)", tela: "Tela",
  provider: "Agente", status: "Status", cliente: "App", detalhe: "Detalhe" };

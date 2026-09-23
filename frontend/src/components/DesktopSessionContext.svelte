<script module lang="ts">
  // Aba escolhida POR SESSÃO. `ctxPanel.aba` é uma só pro app inteiro — sem esta memória, passar
  // por uma sessão sem navegador zerava a aba (o guard logo abaixo) e a sessão de origem voltava
  // em Contexto, com o navegador vivo e escondido atrás dela.
  //
  // Mora no módulo, e não na instância, porque este painel NÃO remonta na troca de sessão: só o
  // `navChave` muda. Foi o que derrubou a primeira tentativa, feita com onMount/onDestroy — ela
  // nunca reexecutava, e a aba continuava se perdendo.
  const ABA_POR_SESSAO = new Map<string, 'contexto' | 'arquivos' | 'navegador' | 'atividade'>();
</script>

<script lang="ts">
  import { ctxPanel, alternarCtxPanel, alternarColunaGit, arrastarLargura, salvarLargura } from '../lib/ctxPanel.svelte';
  import { navegadorPanel, arrastarNav, salvarNav } from '../lib/navegadorPanel.svelte';
  import { workspaceSessionKey } from '../lib/workspaceCommands';
  import NavegadorPane from './NavegadorPane.svelte';
  import ActivitySheet from './ActivitySheet.svelte';
  import { gitPainel } from '../lib/gitPainel.svelte';
  import GitPainelAbas from './git/GitPainelAbas.svelte';
  import MoverBloco from './MoverBloco.svelte';
import * as m from '../paraglide/messages';
import GroupGlyph from './icons/GroupGlyph.svelte';
  import HangarWorking from './icons/HangarWorking.svelte';
  import RateChips from './RateChips.svelte';
  import FilesPanel from './files/FilesPanel.svelte';
  import StateChip from './StateChip.svelte';
  import type { Provider, State, SessionInfo, PlanDetail, ChatEvent, Activity, ShellVivo } from '@hangar/core';
  import type { StatusFields, Shortcut, ShortcutSendText, ShortcutShell } from '@hangar/core';
  import { comTeto, ctxWindow, defaultShortcuts, getSessionCostForServer, providerName, type SessionCostEstimate } from '@hangar/core';
  import ShortcutIcon from './icons/ShortcutIcon.svelte';
  import { listServers } from '../lib/auth';
  import { money2 } from '../lib/fmt';
  import { moeda } from '../lib/moeda.svelte';
  import { criarArquivosMudados } from '../lib/arquivosMudados.svelte';
  import FileIcon from './files/FileIcon.svelte';

  interface Props {
    state: State;
    stateDetail?: string | null;
    status?: StatusFields | null;
    pairPeers?: string[] | null;
    serverLabel?: string;
    provider?: Provider;
    // Identidade do servidor (B2 do parecer): o FilesPanel chaveia o store por
    // serverId::sessionName; o Chat passa o MESMO getActiveId que ele usa, nunca calculado
    // diferente por caller.
    serverId?: string;
    // Nome da sessao aberta: com a NavBar escondida (>=1280px) ele some da tela — e no overlay
    // do board/canvas nem a lista lateral esta a vista. Vai no header do painel.
    sessionName?: string;
    // Progresso do plano (Task 5b): a SessionInfo completa (o PlanPanel/PlanBar leem plan_* dela)
    // + o detalhe de Task/Step, buscado à parte pelo Chat.
    session?: SessionInfo | null;
    planDetail?: PlanDetail | null;
    planLoading?: boolean;
    planError?: boolean;
    // ── Acoes da NavBar ──────────────────────────────────────────────────────
    // No desktop LARGO (>=1280px, mesma breakpoint deste painel) o Chat esconde a NavBar — a
    // informacao dela ja vivia toda aqui — e as ACOES migram pra faixa sob o header. Todas
    // opcionais: sem handler, sem botao (mesma regra da NavBar).
    onOpenTerminal?: () => void;
    terminalAlert?: boolean;
    // Troca terminal ⇄ sem terminal (só Claude). O rótulo é o destino; bloqueado fora de ociosa.
    onTrocarModo?: () => void;
    modoDestinoTerminal?: boolean;
    modoBloqueado?: boolean;
    // Navegador embutido: o botão na fileira de ações ATIVA a aba (criando o navegador da sessão
    // se não tem). A aba Navegador na tab bar só EXISTE quando a sessão tem navegador aberto.
    onOpenNavegador?: () => void;
    onOpenRun?: () => void;
    runRunning?: boolean;
    onOpenAttachments?: () => void;
    // Fileira configurável: a ORDEM e a presença dos botões vêm daqui (lib/shortcuts.svelte.ts,
    // via Chat); ausente = conjunto nativo. Interno sem handler continua sem botão — a lista
    // manda na ordem, os gates de headless/estado continuam mandando na existência.
    shortcuts?: Shortcut[];
    onShortcut?: (s: ShortcutSendText | ShortcutShell) => void;
    onEditShortcuts?: () => void;
    onOpenActivity?: () => void;
    // Atividade como ABA daqui (desktop): é estado ao vivo, como o Navegador, e no modal central
    // ela nascia espremida — o conteúdo é do celular, onde a caixa é a tela toda.
    activity?: Activity | null;
    processos?: ShellVivo[];
    abrirAgente?: { prompt?: string; titulo: string } | null;
    onExpandUsage?: () => void;
    limited?: boolean;
    limitReset?: string | null;
    // Sinal "rodando": hairline varrendo o topo do painel (a work-sweep saiu da NavBar junto).
    working?: boolean;
    // Chip do loop (🔁 N/M) — morava na NavBar; aqui vai junto do estado.
    loopLabel?: string | null;
    loopColor?: string;
    // Follow-up visual: existe um toggle do painel FORA dele (barra de abas no modo 'tabs' OU
    // rodapé do rail no modo 'rail' com a sidebar recolhida) — este painel NÃO renderiza o próprio
    // .ctx-fold (sem duplicação) e, recolhido, SOME em vez de virar aba vertical. Sem toggle
    // externo (sidebar expandida), a porta acessível do painel é preservada (aba vertical).
    toggleExterno?: boolean;
    onLoopTap?: () => void;
    // Codex: tocar o provider abre os limites de uso (na NavBar era o badge tappavel).
    onProviderTap?: () => void;
    // Grupo pareado -> PairSheet (conversa do par, contrato compartilhado, split). A secao dizia
    // "2 sessoes pareadas" e parava ali; a tela do par ja existia, so nao tinha porta aqui.
    onOpenPair?: () => void;
    // Grupo -> modal Orquestração (quem roda cada papel, contas liberadas).
    onOpenOrq?: () => void;
    // Repositorio -> modal de git do cwd. Mesmo caso: dado sem porta.
    onOpenGit?: () => void;
    // Claude sem terminal com MCP/hooks/settings mudados depois da subida: o aviso é ACIONÁVEL e
    // é estado da sessão, então mora aqui. Com o painel aberto o Chat esconde a pill flutuante.
    recarregarMotivo?: string | null;
    onRecarregar?: () => void;
    recarregarBloqueado?: boolean;
    // Clique num arquivo de "Mais alterados" -> abre no editor do app. Sem handler a linha fica
    // só informativa (o modal do Chat não empilha editor dentro de modal).
    onAbrirArquivo?: (path: string) => void;
    // Saídas do contexto cheio. Compactar preenche `/compact` no composer (destrutivo passa pela
    // revisão de quem clicou); passar o bastão abre a folha de criar sessão em modo continuação.
    onCompactar?: () => void;
    onPassarBastao?: () => void;
    // Abre a sessao do MEMBRO num modal (PairChatModal). So com UM par: com 2+ nao da pra escolher
    // por quem clicou, entao a secao segue abrindo a PairSheet, que tem o botao por membro.
    // `undefined` quando o Chat esta `nested` (dentro de um modal) — a guarda que evita modal
    // dentro de modal, e com ela SSE empilhado.
    onOpenPeerChat?: (peer: string) => void;
    // Pra visão "Citados" da aba Arquivos (vêm do Chat).
    events?: ChatEvent[] | null;
    histGap?: string;
    cwd?: string | null;
  }

  let {
    state: sessionState, stateDetail = null, status = null, pairPeers = null,
    events = null, histGap = '', cwd = null,
    serverLabel = '', provider = 'claude', sessionName = '', serverId = '',
    onOpenTerminal = undefined, terminalAlert = false,
    onTrocarModo = undefined, modoDestinoTerminal = false, modoBloqueado = false,
    onOpenNavegador = undefined,
    onOpenRun = undefined, runRunning = false,
    onOpenAttachments = undefined,
    shortcuts = undefined, onShortcut = undefined, onEditShortcuts = undefined,
    onOpenActivity = undefined,
    activity = null, processos = [], abrirAgente = null,
    onExpandUsage = undefined, limited = false, limitReset = null,
    working = false,
    loopLabel = null, loopColor = undefined, onLoopTap = undefined,
    onProviderTap = undefined, onOpenPair = undefined, onOpenOrq = undefined, onOpenGit = undefined,
    recarregarMotivo = null, onRecarregar = undefined, recarregarBloqueado = false,
    onAbrirArquivo = undefined, onCompactar = undefined, onPassarBastao = undefined,
    onOpenPeerChat = undefined,
    session = null, planDetail = null, planLoading = false, planError = false,
    toggleExterno = false,
  }: Props = $props();

  // Só o que dá pra renderizar: interno cujo handler o Chat não passou (headless sem terminal,
  // por exemplo) sai da lista — a config diz a ordem, o gate diz a existência.
  const visibleShortcuts = $derived((shortcuts ?? defaultShortcuts()).filter((s) => {
    if (s.type !== 'internal') return true;
    switch (s.action) {
      case 'terminal': return !!onOpenTerminal;
      case 'modo': return !!onTrocarModo;
      case 'navegador': return !!onOpenNavegador;
      case 'anexos': return !!onOpenAttachments;
      case 'rodar': return !!onOpenRun;
    }
  }));
  const hasActions = $derived(visibleShortcuts.length > 0);
  const navChave = $derived(workspaceSessionKey({ serverId, name: sessionName }));
  // A aba Navegador só existe na tab bar quando a sessão TEM navegador aberto (quem cria é o
  // botão da fileira ou o agente via hangar-preview open).
  const temNav = $derived(navChave in navegadorPanel.abertos);
  // Mesma regra do Navegador: a aba só existe quando há o que mostrar, senão vira uma aba morta
  // em toda sessão. `onOpenActivity` é o gate que o Chat já calcula (tarefas, agentes, subagentes
  // no disco), e processo vivo entra junto.
  const temAtividade = $derived(!!activity && (!!onOpenActivity || processos.length > 0));
  // Qual sessão este painel já viu. Por INSTÂNCIA, não no módulo: hoje só existe um painel montado
  // por vez (no split os Chat extras não recebem showContextPanel, e o overlay é ramo `:else if`),
  // mas com a marca no módulo dois painéis vivos brigariam — um deles nunca casaria a chave e
  // ficaria forçando a própria aba por cima da do irmão. O Map continua no módulo de propósito: ele
  // é a memória por sessão, e é ela que precisa sobreviver a uma remontagem.
  let chaveVista: string | null = null;
  // Mesma sessão: a aba de agora é a escolha dela, guarda. Sessão nova: devolve a aba em que ela
  // estava. Lê as duas coisas no topo de propósito — o efeito precisa acordar tanto na troca de
  // sessão quanto no clique de aba, e ler só dentro de um ramo perderia uma das duas.
  $effect(() => {
    const chave = navChave;
    const aba = ctxPanel.aba;
    if (chave === chaveVista) {
      ABA_POR_SESSAO.set(chave, aba);
      return;
    }
    chaveVista = chave;
    // Sessão que nunca escolheu aba mas já tem navegador (o agente abriu com ela fora da tela):
    // entra direto nele — é o que "o navegador está aberto lá quando eu for" quer dizer.
    const lembrada = ABA_POR_SESSAO.get(chave) ?? (temNav ? 'navegador' : 'contexto');
    if (lembrada !== aba) ctxPanel.aba = lembrada;   // reentra uma vez e cai no ramo de cima
  });
  // A aba é global (ctxPanel, por desenho) mas o navegador é POR SESSÃO: sessão sem navegador com
  // a aba ativa volta pra Contexto — senão a coluna fica em branco (os três painéis têm guard, e
  // header/fileira somem pelo mesmo motivo). Medido no app dele: troca de sessão com a aba ativa
  // deixava a coluna vazia.
  $effect(() => {
    if (ctxPanel.aba === 'navegador' && !temNav) ctxPanel.aba = 'contexto';
  });
  // Mesmo guard pra Atividade, pelo mesmo motivo: a atividade termina (ou a sessão trocada não tem
  // nenhuma) e a aba fica selecionada sobre uma coluna que não desenha mais nada.
  $effect(() => {
    if (ctxPanel.aba === 'atividade' && !temAtividade) ctxPanel.aba = 'contexto';
  });
  // Abas de git abertas pela coluna, e só se forem desta sessão: mostrar o diff do repo anterior
  // depois de trocar de sessão seria outro projeto na tela sem nada avisando.
  const gitAberto = $derived(
    ctxPanel.colunaGit && !!sessionName && gitPainel.sessao === sessionName && gitPainel.abas.length > 0,
  );
  // Atalho da secao Grupo: com UM par, tocar abre a sessao dele direto no modal. Com 2+ membros a
  // secao continua abrindo a PairSheet — la existe o botao por membro, e escolher por quem clicou
  // seria adivinhacao.
  const soloPeer = $derived(pairPeers?.length === 1 ? pairPeers[0] : null);
  // Atalho direto pro modal SO quando ha um par; a PairSheet (contrato, conversa do grupo, lado a
  // lado, sair) nunca perde a porta — vira um segundo botao "grupo" na mesma secao.
  const openPeer = $derived(soloPeer && onOpenPeerChat ? () => onOpenPeerChat(soloPeer) : null);
  const openGroup = $derived(openPeer ?? onOpenPair);
  // RateChips se auto-esconde quando nao tem dial nem banner; a SECAO precisa do mesmo guarda,
  // senao sobra um rotulo "Limites" orfao. isFinite como o known() do RateChips: uma statusline
  // custom que escreva NaN/Infinity nao abre a secao pra um corpo vazio.
  const _known = (pct: number | undefined) => typeof pct === 'number' && isFinite(pct);
  const hasRate = $derived(limited || _known(status?.fiveHourPct) || _known(status?.weeklyPct) || _known(status?.monthlyPct));

  // Contexto perto do teto. O corte é PORCENTAGEM da janela, não um número de tokens: 60% é o
  // "600k de 1M" onde a conversa começa a se arrastar, e vale igual numa sessão de 200k, onde
  // 600k nunca chegaria. Acima de 85% o aviso fica vermelho — ali a compactação automática do
  // próprio agente está a poucos turnos.
  const contextoCheio = $derived.by(() => {
    const p = status?.ctxPct;
    if (typeof p !== 'number' || !isFinite(p)) return null;
    return p >= 85 ? 'grave' : p >= 60 ? 'atencao' : null;
  });

  // Mesmos limiares do resto do app (RateChips, ContextRing): 70 ambar, 90 vermelho. Um vocabulario
  // so de medidor — a barra de contexto era a unica que ficava accent ate os 100%.
  const ctxTone = $derived.by(() => {
    const p = status?.ctxPct;
    if (typeof p !== 'number' || !isFinite(p)) return 'ok';
    return p >= 90 ? 'hot' : p >= 70 ? 'warn' : 'ok';
  });
  // Abaixo de 1k o ctxWindow arredondaria "590" pra "1k" (ele pensa em janelas de modelo, nao em
  // contagens pequenas do turno).
  function tokenShort(n: number): string {
    return n < 1000 ? String(Math.round(n)) : ctxWindow(n);
  }

  let costSnapshot = $state<{ key: string; value: SessionCostEstimate | null; error: string | null } | null>(null);
  let wideEnough = $state(typeof window !== 'undefined' && window.innerWidth >= 1280);
  const costKey = $derived(`${serverId}::${sessionName}::${session?.jsonl ?? ''}`);
  const codexCost = $derived(costSnapshot?.key === costKey ? costSnapshot : null);
  $effect(() => {
    const key = costKey;
    if (provider !== 'codex' || !serverId || !sessionName || !session?.jsonl || !wideEnough || ctxPanel.recolhido
        || ctxPanel.aba !== 'contexto' || gitAberto) return;
    const server = listServers().find((item) => item.id === serverId);
    if (!server) return;
    let controller: AbortController | null = null;
    let lastValue: SessionCostEstimate | null = null;
    const load = async () => {
      controller?.abort();
      const current = new AbortController();
      controller = current;
      try {
        const value = await getSessionCostForServer(server, sessionName, comTeto(current.signal, 25_000));
        if (!current.signal.aborted) {
          lastValue = value;
          costSnapshot = { key, value, error: null };
        }
      } catch (error) {
        if (!current.signal.aborted) {
          costSnapshot = { key, value: lastValue, error: error instanceof Error ? error.message : String(error) };
        }
      }
    };
    void load();
    const timer = window.setInterval(() => { void load(); }, 30_000);
    return () => { window.clearInterval(timer); controller?.abort(); };
  });

  // Claude publica o custo na statusline; Codex usa o histórico da sessão e a tarifa de API.
  const costAmount = $derived(provider === 'codex' ? codexCost?.value?.cost_usd : status?.costUsd);
  const custoLabel = $derived(
    typeof costAmount === 'number' && isFinite(costAmount)
      ? money2(costAmount, moeda.cur, moeda.rate)
      : null,
  );
  const custoTitle = $derived.by(() => {
    if (provider !== 'codex') return undefined;
    if (codexCost?.error) return codexCost.error;
    const missing = codexCost?.value?.missing_models ?? [];
    return missing.length ? `${m.custos_sem_tarifa_volume()}: ${missing.join(', ')}` : m.custos_aviso_estimativa();
  });
  const costCaption = $derived.by(() => {
    if (provider !== 'codex') return m.ctx_nesta_sessao();
    return codexCost?.value?.missing_models.length ? m.custos_sem_tarifa() : m.custos_estimativa_api();
  });
  $effect(() => {
    if (costAmount != null) moeda.garantirCotacao();
  });
  // Duracao CRUA ("12min", "3h", "2d"), nao o relativeTime do core: ele ja embute o "atras", e a
  // frase daqui e "trabalhando ha {t}" — as duas juntas davam "trabalhando ha 12 min atras".
  // Os sufixos sao os mesmos nos dois idiomas, entao nao viram chave.
  function duracaoCurta(desde: number): string {
    const s = Math.max(0, Date.now() / 1000 - desde);
    if (s < 60) return '<1min';
    if (s < 3600) return `${Math.floor(s / 60)}min`;
    if (s < 86400) return `${Math.floor(s / 3600)}h`;
    return `${Math.floor(s / 86400)}d`;
  }
  // Há quanto tempo a sessão está PARADA — o que a tela não dizia em lugar nenhum.
  //
  // Trabalhando não entra aqui de propósito: o próprio agente já publica a duração do turno no
  // detalhe do estado, no header deste painel ("Boogieing… (46m 15s)"). Um segundo relógio contado
  // daqui discordava dele na mesma tela (contava do último prompt, não do começo do turno).
  const tempoNoEstado = $derived.by(() => {
    if (sessionState === 'working') return null;
    const ts = session?.last_activity;
    return ts ? m.ctx_parada_ha({ t: duracaoCurta(ts) }) : null;
  });
  // Fila: as bolhas `queued-` que ainda nao foram confirmadas pelo transcript. O painel ja recebe
  // os eventos do Chat, entao a contagem nao custa uma rota nova.
  const naFila = $derived(
    events?.filter((e) => e.id.startsWith('queued-') && !e.queued_confirmed).length ?? 0,
  );
  // Diff do working tree vs HEAD: o backend ja calcula (git_ops.git_diffstat) e manda na lista.
  // "alteracoes locais" dizia que havia algo; isto diz quanto.
  const diffRepo = $derived.by(() => {
    const a = session?.git_added, r = session?.git_removed;
    if (typeof a !== 'number' && typeof r !== 'number') return null;
    return { added: a ?? 0, removed: r ?? 0, files: session?.git_dirty ?? 0 };
  });

  // QUAIS arquivos mudaram, nao so quantos: as tres maiores mudancas, direto no Contexto. A lista
  // inteira continua a um clique (o painel de git). A chave carrega os contadores do git que a
  // listagem ja traz — mexeu no repo, muda a chave, recarrega; parado, nenhuma chamada.
  const arqMudados = criarArquivosMudados();
  $effect(() => {
    const chave = sessionName && status?.repo && session?.git_dirty
      ? `${sessionName}:${session.git_dirty}:${session.git_added ?? 0}:${session.git_removed ?? 0}`
      : '';
    void arqMudados.carregar(sessionName, chave);
  });
  // "src/components/X.svelte" -> ".../X.svelte": o nome do arquivo é o que se reconhece, e a
  // coluna não comporta o caminho inteiro.
  function caminhoCurto(p: string): string {
    const partes = p.split('/');
    return partes.length <= 2 ? p : `…/${partes.slice(-2).join('/')}`;
  }
  // ── Largura redimensionavel (drag na divisória da esquerda), persistida ─────────────
  // Mesma pegada da Sidebar (cp_sidebar_w): pointer capture no handle, largura clampsa no
  // store (arrastarLargura), salva no soltar. Sem transicao de width no painel, o arrasto
  // segue o ponteiro sem lag — a classe resizing fica como trava contra alguém adicionar
  // transicao depois (mesmo motivo do .sidebar.resizing). O flag `resizing` vive no store
  // (ctxPanel) porque a largura e compartilhada com o DesktopShell.
  function resizeStart(e: PointerEvent) {
    ctxPanel.resizing = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function resizeMove(e: PointerEvent) {
    if (!ctxPanel.resizing) return;
    // Com a aba Navegador ativa a divisória mexe na largura DELE (store próprio, teto próprio) —
    // a coluna engrossa pro browser; nas outras abas, a do contexto como sempre.
    // A borda direita sai da COLUNA, não da janela: o painel não cola mais no lado da tela (pode
    // ter a conversa ou o git à direita) e o que se arrasta é a largura da coluna, que inclui a
    // folga do card — medir pelo painel encolhia 24px a cada volta do arrasto.
    const alca = e.currentTarget as HTMLElement;
    const direita = (alca.closest('.ctx-slot') ?? alca.closest('.session-context'))
      ?.getBoundingClientRect().right ?? window.innerWidth;
    if (ctxPanel.aba === 'navegador') arrastarNav(e.clientX, direita);
    else arrastarLargura(e.clientX, direita);
  }
  function resizeEnd() {
    if (!ctxPanel.resizing) return;
    ctxPanel.resizing = false;
    if (ctxPanel.aba === 'navegador') salvarNav();
    else salvarLargura();
  }
  // O flag vive no store (singleton de modulo) e sobrevive à desmontagem. Se a alca sair do DOM
  // no meio do arrasto — recolher, cruzar os 820px, trocar de sessao — o pointerup nao tem
  // destino e o resizing ficaria preso, fazendo a divisoria redimensionar so com o cursor por cima.
  // Dois bracos, porque sao dois mecanismos de saida: recolher NAO desmonta o componente (a alca
  // some por {#if} interno) — o flag zera quando recolhido vira true; os outros caminhos
  // (820px, troca de sessao) desmontam o componente inteiro — o cleanup zera na desmontagem.
  $effect(() => {
    if (ctxPanel.recolhido) ctxPanel.resizing = false;
    return () => { ctxPanel.resizing = false; };
  });
</script>

<svelte:window onresize={() => (wideEnough = window.innerWidth >= 1280)} />

<aside class="session-context" class:recolhido={ctxPanel.recolhido} class:toggle-externo={toggleExterno} class:resizing={ctxPanel.resizing} aria-label={m.ctx_painel_titulo()}>
  {#if !ctxPanel.recolhido}
  <!-- Drag na divisória da esquerda pra redimensionar: mesma pegada do resize-handle da Sidebar
       (lá a borda é a direita, aqui a esquerda — o painel cola na direita). So quando aberto: o
       trilho recolhido não é largura, é estado. -->
  <div class="ctx-resize-handle" role="separator" aria-label={m.ctx_redimensionar()}
    aria-orientation="vertical"
    onpointerdown={resizeStart} onpointermove={resizeMove}
    onpointerup={resizeEnd} onpointercancel={resizeEnd}></div>
  {/if}
  <!-- Botão de recolher: com toggle externo (barra no modo tabs OU rail recolhido) o controle mora lá — aqui fica
       sem botão duplicado. Sem a barra, o botão do topo é a porta acessível dos dois sentidos. -->
  {#if !toggleExterno}
  <button class="ctx-fold" onclick={alternarCtxPanel}
          aria-label={ctxPanel.recolhido ? m.ctx_expandir_contexto() : m.ctx_recolher_contexto()}
          title={ctxPanel.recolhido ? m.sessao_expandir() : m.sessao_recolher()}>
    <!-- MESMO ícone do recolher da barra esquerda: os dois fazem a mesma coisa em lados opostos,
         então usar desenhos diferentes obrigava a reaprender o controle de cada lado. -->
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="2"/>
      <line x1="9" y1="4" x2="9" y2="20"/>
    </svg>
  </button>
  {/if}

  {#if ctxPanel.recolhido}
    <!-- Recolhido = ESCONDIDO, não trilho. O trilho foi tentado e não se pagou: em 50px cabia só um
         texto vertical e um anel, enquanto o valor do painel é o plano, as ações e as métricas —
         coisas que precisam de largura. Estado e progresso continuam à vista na barra da esquerda. -->
  {:else}
  <!-- Na aba Navegador a varredura de "sessão trabalhando" some: em cima de uma página web ela
       lê como "a página está carregando" — e o carregamento de verdade tem barra própria lá. -->
  {#if working && ctxPanel.aba !== 'navegador'}<div class="ctx-sweep" aria-hidden="true"></div>{/if}
  <!-- Com a aba Navegador ativa o header some: o browser ganha a altura (pedido dele). -->
  {#if ctxPanel.aba !== 'navegador'}
  <header>
    <div class="ctx-heading">
      <!-- Sem kicker: "Contexto da sessão" repetia o que o painel inteiro e (e ja esta no
           aria-label do <aside>). O nome da sessao e o titulo real. -->
      {#if sessionName}<strong class="header-session" title={sessionName}>{sessionName}</strong>{/if}
      {#if stateDetail}<p class="header-detail">{stateDetail}</p>{/if}
    </div>
    <div class="header-right">
      <MoverBloco bloco="ctx" />
      <StateChip state={sessionState} size="md" />
      {#if loopLabel}
        <button type="button" class="loop-chip" style="color: {loopColor};" onclick={onLoopTap} aria-label={m.ctx_aria_loop({ n: loopLabel })}>{loopLabel}</button>
      {/if}
    </div>
  </header>
  {/if}

  <!-- Barra de abas do painel (Contexto | Arquivos), no desenho do mock aprovado. A aba ativa
       vive no ctxPanel (modulo): o App remonta este painel por {#key} a cada troca de sessao,
       e um $state local devolveria o usuario pra Contexto com Arquivos aberta. Recolhido, o
       painel some e a barra some junto — sem porta fantasma. -->
  <!-- Acoes da sessao (Terminal, Rodar, Anexos, Atividade) ficam ACIMA das abas: valem pra
       sessao inteira, nao pra aba Contexto — e ninguem devia trocar de aba pra achar o Terminal.
       Com a aba Navegador ativa a fileira SOME: quem ta ali ta mexendo no browser, e o browser
       ganha a altura. O Navegador nao e mais acao — e a aba ao lado. -->
  {#if hasActions && ctxPanel.aba !== 'navegador'}
    <!-- Clique-direito abre o editor de atalhos: a fileira é configurável e o caminho de edição
         mora na config — este é o acesso rápido de quem já está olhando pra ela. -->
    <!-- tabindex -1: o contextmenu torna a toolbar "interativa" pro linter a11y, mas o foco de
         teclado pertence aos botões dela — a toolbar em si não é parada de Tab. -->
    <div class="ctx-actions" role="toolbar" aria-label={m.ctx_painel_titulo()} tabindex="-1"
         oncontextmenu={onEditShortcuts ? (e) => { e.preventDefault(); onEditShortcuts(); } : undefined}>
      {#each visibleShortcuts as s (s.id)}
        {#if s.type === 'internal' && s.action === 'terminal'}
          <button class="ctx-action terminal-btn" class:alert={terminalAlert} onclick={onOpenTerminal} aria-label={m.ctx_terminal()}>
            <span class="animated-icon" aria-hidden="true">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <rect x="2.5" y="4" width="19" height="16" rx="2"/>
                <path d="M6.5 9l3 3-3 3"/>
                <line x1="12.5" y1="15" x2="17" y2="15"/>
              </svg>
            </span>
            <span>{m.ctx_terminal()}</span>
          </button>
        {:else if s.type === 'internal' && s.action === 'modo'}
          {@const rotulo = modoDestinoTerminal ? m.modo_abrir_no_terminal() : m.modo_continuar_sem_terminal()}
          <button class="ctx-action" onclick={onTrocarModo} disabled={modoBloqueado} aria-label={rotulo}
                  title={modoBloqueado ? m.modo_so_ociosa() : modoDestinoTerminal ? m.modo_abrir_no_terminal_detalhe() : m.modo_continuar_sem_terminal_detalhe()}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M4 8h13l-3-3"/>
              <path d="M20 16H7l3 3"/>
            </svg>
            <span>{rotulo}</span>
          </button>
        {:else if s.type === 'internal' && s.action === 'navegador'}
          <button class="ctx-action" onclick={onOpenNavegador} aria-label={m.ctx_navegador()}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <circle cx="12" cy="12" r="9"/>
              <path d="M3 12h18"/>
              <path d="M12 3c2.5 2.6 3.9 5.7 3.9 9s-1.4 6.4-3.9 9c-2.5-2.6-3.9-5.7-3.9-9s1.4-6.4 3.9-9z"/>
            </svg>
            <span>{m.ctx_navegador()}</span>
          </button>
        {:else if s.type === 'internal' && s.action === 'anexos'}
          <button class="ctx-action" onclick={onOpenAttachments} aria-label={m.ctx_anexos_da_sessao()}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M21 11l-8.5 8.5a5 5 0 0 1-7-7L14 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L16 6"/>
            </svg>
            <span>{m.ctx_anexos()}</span>
          </button>
        {:else if s.type === 'internal' && s.action === 'rodar'}
          <!-- Rodar atrás de um divisor: os outros abrem um painel, este dispara um processo no
               projeto. Só quando não é o primeiro — divisor abrindo a fileira é ruído. -->
          {#if visibleShortcuts[0] !== s}
            <span class="acao-divisor" aria-hidden="true"></span>
          {/if}
          <button class="ctx-action run-btn" class:running={runRunning} onclick={onOpenRun}
                  aria-label={runRunning ? m.ctx_rodando_abrir() : m.ctx_rodar_projeto()}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              {#if runRunning}
                <rect x="6" y="6" width="12" height="12" rx="2" />
              {:else}
                <path d="M8 5v14l11-7z" />
              {/if}
            </svg>
            <span>{runRunning ? m.ctx_rodando() : m.ctx_rodar()}</span>
          </button>
        {:else if s.type === 'send_text' || s.type === 'shell'}
          <button class="ctx-action" onclick={() => onShortcut?.(s)} aria-label={s.label} title={s.type === 'shell' ? s.command : s.text}>
            <ShortcutIcon icon={s.icon} />
            <span>{s.label}</span>
          </button>
        {/if}
      {/each}
    </div>
  {/if}

  <div class="abas" role="tablist" aria-label={m.ctx_painel_titulo()}>
    <button type="button" id="aba-ctx-contexto" class="aba" class:sel={ctxPanel.aba === 'contexto'}
            role="tab" aria-selected={ctxPanel.aba === 'contexto'} aria-controls="painel-ctx-contexto"
            onclick={() => (ctxPanel.aba = 'contexto')}>
      {m.ctx_aba_contexto()}
    </button>
    <button type="button" id="aba-ctx-arquivos" class="aba" class:sel={ctxPanel.aba === 'arquivos'}
            role="tab" aria-selected={ctxPanel.aba === 'arquivos'} aria-controls="painel-ctx-arquivos"
            onclick={() => (ctxPanel.aba = 'arquivos')}>
      {m.arq_aba()}
    </button>
    {#if temNav}
    <button type="button" id="aba-ctx-navegador" class="aba" class:sel={ctxPanel.aba === 'navegador'}
            role="tab" aria-selected={ctxPanel.aba === 'navegador'} aria-controls="painel-ctx-navegador"
            onclick={() => (ctxPanel.aba = 'navegador')}>
      {m.ctx_navegador()}
    </button>
    {/if}
    {#if temAtividade}
    <button type="button" id="aba-ctx-atividade" class="aba" class:sel={ctxPanel.aba === 'atividade'}
            role="tab" aria-selected={ctxPanel.aba === 'atividade'} aria-controls="painel-ctx-atividade"
            onclick={() => (ctxPanel.aba = 'atividade')}>
      {m.ctx_atividade()}
    </button>
    {/if}
    <!-- Git NÃO é aba deste painel: abre a coluna à esquerda da conversa e deixa Contexto/Arquivos
         no lugar. Por isso é um toggle (aria-pressed), não um role="tab".
         `status?.repo`: sessão cujo cwd não é repositório não ganha o botão — abrir a coluna lá só
         mostrava o "fatal: not a git repository" do git. -->
    {#if status?.repo}
    <button type="button" class="aba git-toggle" class:sel={ctxPanel.colunaGit}
            aria-pressed={ctxPanel.colunaGit}
            onclick={alternarColunaGit}>
      {m.git_coluna_abrir()}
    </button>
    {/if}
  </div>

  {#if gitAberto && sessionName}
  <!-- Abas de git: o que a coluna abriu (arquivo ou commit). Fica POR CIMA de Contexto/Arquivos,
       não vira uma quarta aba — fechar a última devolve o painel exatamente como estava. -->
  <div class="ctx-tab">
    <GitPainelAbas {sessionName} />
  </div>
  {/if}

  {#if !gitAberto && ctxPanel.aba === 'navegador' && temNav}
  <!-- O navegador é uma ABA da coluna: trocar pra Contexto/Arquivos esconde o view (desmonta o
       painel -> nav-hide; o agente segue usando via CDP), nunca fecha. O × dele é quem fecha. -->
  <div id="painel-ctx-navegador" role="tabpanel" aria-labelledby="aba-ctx-navegador" class="ctx-tab ctx-tab-nav">
    <NavegadorPane navKey={navChave} />
  </div>
  {/if}

  {#if !gitAberto && ctxPanel.aba === 'atividade' && temAtividade && activity}
  <!-- Mesmo corpo do modal do celular, sem o embrulho de dialog: aqui ele tem a altura da coluna,
       que é o que faltava pro detalhe de um agente caber sem virar caixinha. -->
  <div id="painel-ctx-atividade" role="tabpanel" aria-labelledby="aba-ctx-atividade" class="ctx-tab">
    <ActivitySheet docado open {activity} sessionName={sessionName ?? ''} {processos} {abrirAgente}
                   onClose={() => (ctxPanel.aba = 'contexto')} session={session} {planDetail} {planLoading} {planError} />
  </div>
  {/if}

  {#if !gitAberto && ctxPanel.aba === 'contexto'}
  <div id="painel-ctx-contexto" role="tabpanel" aria-labelledby="aba-ctx-contexto" class="ctx-tab">

  <!-- A secao "Estado" saiu: repetia o chip do header a 60px de distancia, mesma palavra e mesma
       cor. O detalhe e o chip do loop subiram pro header, que ja era o lugar do estado. -->

  <div class="ctx-scroll">
  <!-- AGORA: o que muda sozinho enquanto a sessao trabalha — contexto, custo, tempo e turno —
       num bloco so, com o numero grande. Antes eram cinco rotulos de secao de peso igual e o
       dado mais importante saia em corpo 11px; aqui ele e a primeira coisa que o olho pega.
       Sem rotulo de secao de proposito: e o topo, nao concorre com os rotulos da lista abaixo. -->
  <section class="sec-agora">
    <div class="agora-topo">
      <span class="agora-ctx">
        <strong class="agora-num tone-{ctxTone}" class:vazio={status?.ctxPct == null}>{status?.ctxPct != null ? `${Math.round(status.ctxPct)}%` : '—'}</strong>
        <span class="agora-cap">
          {m.ctx_do_contexto()}{#if status?.ctxUsed != null && status.ctxTotal}<span class="mono">&nbsp;· {m.ctx_usado_de_total({ usado: ctxWindow(status.ctxUsed), total: ctxWindow(status.ctxTotal) })}</span>{:else if status?.ctxTotal}<span class="mono">&nbsp;· {ctxWindow(status.ctxTotal)}</span>{/if}
        </span>
      </span>
      {#if custoLabel || provider === 'codex'}
        <span class="agora-custo" title={custoTitle}>
          <strong>{custoLabel ?? '—'}</strong>
          <span>{costCaption}</span>
        </span>
      {/if}
    </div>

    {#if status?.ctxPct != null}
      <div class="progress tone-{ctxTone}" aria-label={m.ctx_pct_usado({ n: Math.round(status.ctxPct) })}>
        <span style:width={`${status.ctxPct}%`}></span>
      </div>
    {:else}
      <!-- Vazio EXPLICADO: "medicao indisponivel" sozinho lia como bug do painel. -->
      <p>{m.ctx_medicao_depois_turno()}</p>
    {/if}

    {#if tempoNoEstado || status?.turnIn != null || status?.turnOut != null}
      <p class="agora-linha">
        {#if tempoNoEstado}<span>{tempoNoEstado}</span>{/if}
        {#if status?.turnIn != null || status?.turnOut != null}
          <span class="mono">{m.ctx_ultimo_turno()} {ctxWindow(status.turnIn ?? 0)} {m.ctx_entrada()} · {status.turnOut != null ? tokenShort(status.turnOut) : '—'} {m.ctx_saida()}</span>
        {/if}
      </p>
    {/if}

    {#if hasRate}
      <div class="saude-limites">
        <RateChips {status} onExpand={onExpandUsage} {limited} {limitReset} variant="bars" />
      </div>
    {/if}
  </section>

  {#if recarregarMotivo && onRecarregar}
    <!-- Único aviso ACIONÁVEL do painel: o processo está rodando com MCP/hooks velhos e o conserto
         é um clique. Fica logo abaixo do estado porque é sobre o estado, não sobre o repositório. -->
    <div class="ctx-aviso" role="status">
      <span class="ctx-aviso-texto">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="12" cy="12" r="9" /><path d="M12 8v4.5" /><path d="M12 16h.01" />
        </svg>
        {m.recarregar_aviso_config()}
      </span>
      <button type="button" class="ctx-aviso-btn" onclick={onRecarregar} disabled={recarregarBloqueado}
              title={recarregarBloqueado ? m.modo_so_ociosa() : m.recarregar_sessao_detalhe()}>
        {m.recarregar_agora()}
      </button>
    </div>
  {/if}

  {#if contextoCheio}
    <!-- Contexto perto do teto: a partir daqui a conversa começa a perder qualidade, porque cada
         turno reescreve mais coisa do que o modelo consegue manter em foco. As duas saídas ficam
         aqui do lado do número que disparou o aviso. -->
    <div class="ctx-aviso" class:grave={contextoCheio === 'grave'} role="status">
      <span class="ctx-aviso-texto">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="12" cy="12" r="9" /><path d="M12 8v4.5" /><path d="M12 16h.01" />
        </svg>
        {m.ctx_cheio_aviso()}
      </span>
      <span class="ctx-aviso-acoes">
        {#if onCompactar}
          <!-- Preenche `/compact` no composer em vez de enviar: é comando destrutivo, e a regra do
               app é que destrutivo passa pela revisão de quem clicou. -->
          <button type="button" class="ctx-aviso-btn" onclick={onCompactar} title={m.ctx_cheio_compactar_detalhe()}>
            {m.ctx_cheio_compactar()}
          </button>
        {/if}
        {#if onPassarBastao}
          <button type="button" class="ctx-aviso-btn" onclick={onPassarBastao} title={m.ctx_cheio_bastao_detalhe()}>
            {m.ctx_cheio_bastao()}
          </button>
        {/if}
      </span>
    </div>
  {/if}

  <!-- "alteracoes locais" dizia que HAVIA algo e parava ali; o tamanho da mudanca so aparecia
       abrindo o modal de git. O backend ja manda o numstat na listagem (git_ops.git_diffstat). -->
  {#snippet repoEstado()}
    {#if diffRepo && (diffRepo.added || diffRepo.removed)}
      <p class="repo-diff mono">
        <span class="diff-add">+{diffRepo.added}</span>
        <span class="diff-del">−{diffRepo.removed}</span>
        {#if diffRepo.files}<span class="diff-files">{m.ctx_arquivos_n({ n: diffRepo.files })}</span>{/if}
      </p>
    {:else if status?.dirty}
      <p class="mono">{m.ctx_alteracoes_locais()}</p>
    {/if}
  {/snippet}

  {#if status?.repo}
  <section class="sec-break">
    <span class="section-label">{m.ctx_repositorio()}</span>
    {#if onOpenGit}
      <button type="button" class="sec-open" onclick={onOpenGit} aria-label={m.ctx_abrir_git({ n: status.repo })}>
        <span class="sec-open-body">
          <strong class="mono">{status.repo} · {status.branch ?? m.ctx_sem_branch()}</strong>
          {@render repoEstado()}
        </span>
        <span class="sec-open-arrow" aria-hidden="true">›</span>
      </button>
    {:else}
      <strong class="mono">{status.repo} · {status.branch ?? m.ctx_sem_branch()}</strong>
      {@render repoEstado()}
    {/if}

    <!-- QUAIS arquivos mudaram. Fica dentro de Repositório (é sobre ele) e não vira seção nova:
         a lista inteira é o painel de git, aqui são só as três maiores. -->
    <!-- Mesmo critério do repoEstado: com o repositório limpo `git_added`/`git_removed` chegam
         como 0 (número, não ausente), então `diffRepo` continua existindo — sem checar o conteúdo,
         a lista seguia na tela com os arquivos de antes do commit. -->
    {#if diffRepo && (diffRepo.added || diffRepo.removed) && arqMudados.itens.length}
      <div class="arq-topo">
        <span class="arq-titulo">{m.ctx_mais_alterados()}</span>
        {#if onOpenGit}
          <button type="button" class="arq-todos" onclick={onOpenGit}>{m.ctx_ver_todos()}</button>
        {/if}
      </div>
      <ul class="arq-lista">
        {#each arqMudados.itens as arq (arq.path)}
          <li>
            <button type="button" class="arq-linha" onclick={() => onAbrirArquivo?.(arq.path)}
                    disabled={!onAbrirArquivo} title={arq.path}
                    aria-label={m.ctx_abrir_arquivo({ n: arq.path })}>
              <FileIcon nome={arq.path} />
              <span class="mono arq-path">{caminhoCurto(arq.path)}</span>
              <span class="mono diff-add">+{arq.added}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
  {/if}

  <!-- EQUIPE: Grupo + Orquestração fundidos. Sem par e sem porta de orquestração a seção nem
       existe — era um bloco morto pra quem raramente pareia. A linha de orquestração fica sempre
       que há onOpenOrq: é a única porta pro time padrão sem grupo. -->
  {#if pairPeers?.length || onOpenPair || onOpenOrq}
  <section class="sec-break">
    <span class="section-label">{m.ctx_equipe()}</span>
    {#if pairPeers?.length}
      {#if openGroup}
        <button type="button" class="sec-open" onclick={openGroup}
                aria-label={soloPeer && onOpenPeerChat ? m.ctx_abrir_sessao_modal({ n: soloPeer }) : m.ctx_abrir_par({ n: pairPeers.join(', ') })}>
          <span class="sec-open-body">
            <strong><GroupGlyph size={13} /> {pairPeers.join(' · ')}</strong>
            <p>{soloPeer && onOpenPeerChat ? m.ctx_abrir_conversa_dele() : `${pairPeers.length + 1} ${m.ctx_sessoes_pareadas()}`}</p>
          </span>
          <span class="sec-open-arrow" aria-hidden="true">›</span>
        </button>
        {#if openPeer && onOpenPair}
          <!-- O atalho abre a conversa do par; o grupo em si (contrato, conversa, lado a lado, sair)
               continua a um toque daqui. -->
          <button type="button" class="sec-side" onclick={onOpenPair} aria-label={m.ctx_abrir_grupo_pareado()}>
            {m.ctx_ver_grupo()}
          </button>
        {/if}
      {:else}
        <strong><GroupGlyph size={13} /> {pairPeers.join(' · ')}</strong>
        <p>{pairPeers.length + 1} {m.ctx_sessoes_pareadas()}</p>
      {/if}
    {:else if onOpenPair}
      <button type="button" class="sec-open" onclick={onOpenPair} aria-label={m.ctx_parear_outra()}>
        <span class="sec-open-body">
          <strong>{m.ctx_parear_outra()}</strong>
        </span>
        <span class="sec-open-arrow" aria-hidden="true">›</span>
      </button>
    {/if}
    {#if onOpenOrq}
      <button type="button" class="sec-open" onclick={onOpenOrq} aria-label={m.ctx_orquestracao()}>
        <span class="sec-open-body">
          <strong>{m.ctx_orquestracao_titulo()}</strong>
          <p>{m.ctx_orquestracao_desc()}</p>
        </span>
        <span class="sec-open-arrow" aria-hidden="true">›</span>
      </button>
    {/if}
  </section>
  {/if}

  </div>

  <!-- EXECUÇÃO no RODAPÉ, fora do scroller: provider e máquina não mudam na vida da sessão, e
       como seção irmã das outras gastavam o mesmo rótulo e o mesmo respiro que o que muda.
       Modelo/esforço continuam fora (estão nas pills do composer, onde se troca). A fila entra
       aqui do lado: é do turno, aparece só quando há algo esperando. -->
  <footer class="ctx-rodape">
    {#if onProviderTap}
      <button type="button" class="provider-tap" onclick={onProviderTap} aria-label={m.ctx_limites_provider()}>
        {providerName(provider)}{serverLabel ? ` · ${serverLabel}` : ''}
      </button>
    {:else}
      <span class="rodape-exec">{providerName(provider)}{serverLabel ? ` · ${serverLabel}` : ''}</span>
    {/if}
    {#if naFila > 0}
      <span class="rodape-fila">{m.ctx_na_fila({ n: naFila })}</span>
    {/if}
  </footer>
  </div>
  {:else if ctxPanel.aba === 'arquivos'}
  <div id="painel-ctx-arquivos" role="tabpanel" aria-labelledby="aba-ctx-arquivos" class="ctx-tab">
    <FilesPanel sessionName={sessionName} {serverId} desktop={true} {events} {histGap} {cwd} />
  </div>
  {/if}
  {/if}
</aside>

<style>
  /* O cabeçalho RESERVA a faixa do botão de recolher: sem isso ele cai por cima do chip de estado —
     `position: absolute` não empurra nada. 44px do botão + folga. */
  .session-context:not(.recolhido) header { padding-right: 56px; }
  .session-context {
    /* Coluna: header + acoes ficam PRESOS no topo e so o corpo rola. Antes o painel inteiro era o
       scroller, entao o nome da sessao e o botao Terminal subiam junto com as metricas. */
    display: flex;
    flex-direction: column;
    container-type: inline-size;
    /* COLUNA do shell, não card por cima da conversa: o nó é reparentado pelo Chat pra `.ctx-slot`
       (DesktopShell), que é quem reserva a largura. `relative` fica pelos filhos absolutos daqui
       (o punho de arrastar, os pontinhos de aviso). Enquanto era `absolute`, trocar de lugar com
       a coluna de git era impossível — ele não era irmão de ninguém. */
    position: relative;
    flex: 1;
    min-width: 0;
    z-index: 17;
    /* CAIXA SOLTA, não parede colada na borda (mesma ideia do painel do Gemini no Gmail): folga em
       volta, cantos redondos e sombra, pra ler como uma seção à parte em vez de "o chat encolheu".
       A folga sai de DENTRO da coluna: a largura salva pela pessoa continua sendo a da coluna
       inteira, como era com a faixa reservada pelo Chat. */
    --ctx-gap: var(--space-3);
    /* Folga nos quatro lados: o painel pode ficar em qualquer posição do arranjo, e a margem só
       à direita o deixava encostado no trilho quando ele vai pra esquerda da conversa. */
    margin: var(--ctx-gap);
    overflow: hidden;
    /* MESMA receita da sidebar e da faixa de cota (18/08): eram três acabamentos parecidos e
       nenhum igual — este usava --border-default (mais forte) e uma sombra SEM o brilho de borda
       que os outros dois tinham. A sombra agora é o token --elev-3, um lugar só. */
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-xl);
    box-shadow: var(--elev-3);
    background: transparent;   /* o fundo vai pro leaf ::before (vidro) */
  }

  /* Vidro do painel — mesmo material do composer/navbar/sidebar. Leaf ::before pra o
     backdrop-filter do Chromium nao virar containing block do conteudo. */
  .session-context::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;
    pointer-events: none;
    border-radius: inherit;   /* sem isto o vidro vaza os cantos do card */
    background: var(--glass-bg-solid);
  }
  /* Mesmo motivo do BottomSheet: o filtro vai no ELEMENTO. Num pseudo dentro de um ancestral
     transformado o backdrop root muda e o blur nao pega nada — sobra so a transparencia, com o
     texto do chat atravessando o painel. O painel nao tem filho `position: fixed`, entao virar
     containing block aqui nao quebra nada. */
  :global(html[data-liquid]) .session-context {
    /* `url(#liquid-glass)` junto: sem ele o painel tinha só blur, e ao lado do composer (que refrata)
       parecia outro material. O filtro continua no ELEMENTO pelo motivo do bloco acima. */
    backdrop-filter: url(#liquid-glass) blur(20px) saturate(170%);
  }
  /* Mesmo vidro do composer sob liquid (Sidebar.svelte tem a mesma regra e o porquê): com o filtro
     no elemento e o Chromium honrando, `--glass-panel` só empilhava um 2o fundo e matava a refração.
     Escuro só — no claro 0.52 de branco não cobre texto escuro e o chat atravessa o painel. */
  :global(html[data-liquid]) .session-context::before {
    background: var(--glass-panel);
  }
  :global(html[data-liquid][data-theme='dark']) .session-context::before {
    background: var(--glass-bg);
  }

  /* Aparência → Painéis → "Colados": volta o painel de ponta a ponta, como era antes do card. */
  :global(html[data-panels='edge']) .session-context {
    --ctx-gap: 0px;
    border: 0;
    border-left: 1px solid var(--border-subtle);
    border-radius: 0;
    box-shadow: none;
  }

  /* Enquanto arrasta: sem transicao (segue o ponteiro sem lag). Hoje o painel nao tem
     transition de width; a regra trava contra alguem adicionar depois (mesmo motivo do
     .sidebar.resizing). */
  .session-context.resizing { transition: none; }
  .ctx-resize-handle {
    position: absolute; top: 0; left: 0; width: 6px; height: 100%;
    cursor: col-resize; z-index: 6; touch-action: none;
  }
  @media (hover: hover) {
    .ctx-resize-handle:hover { background: var(--accent-dim); }
  }

  header, .ctx-actions { flex: 0 0 auto; }
  /* Envoltorio de cada aba: o conteudo de contexto e o FilesPanel ocupam a mesma faixa
     flexivel abaixo das abas. O ctx-scroll (rolagem do contexto) fica dentro dele. */
  .ctx-tab {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .commit-topo {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border-subtle);
  }
  .commit-sha { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--accent); }
  .commit-sub {
    font-size: var(--text-xs); color: var(--text-secondary);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .commit-x {
    margin-left: auto; background: none; border: 0; color: var(--text-muted);
    cursor: pointer; padding: 0 var(--space-1); line-height: 1;
  }
  .commit-x:hover { color: var(--text-primary); }
  /* position: relative pela regra do overflow próprio: sem isso um .sr-only absoluto de dentro
     do diff escaparia pra área rolável da conversa. */
  .commit-scroll { flex: 1; min-height: 0; overflow: auto; position: relative; }
  /* A aba Navegador deixa 8px à esquerda: o handle de redimensionar da coluna (absolute, left:0,
     6px) precisa ficar FORA do view nativo, senão o view cobre a divisória e o clique morre —
     era o "depois que abre uma página não dá pra redimensionar". */
  .ctx-tab-nav { margin-left: 8px; }
  .ctx-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  header {
    min-height: 64px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    /* A partir de 1280px com este painel aberto a NavBar some (Chat.svelte) e quem encosta na
       borda direita da janela é este header — logo é ele que abre a faixa dos controles da
       janela no PWA em window-controls-overlay. Zero fora desse modo. */
    padding: var(--space-2) calc(var(--space-4) + var(--cp-wco-right)) var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
    background: color-mix(in srgb, var(--bg-elevated) 52%, transparent);
  }

  /* Piso de largura: com as setas de arranjo no canto direito, o título era o único item elástico
     e cedia tudo — o nome da sessão virava "han…" num painel de 316px. */
  .ctx-heading {
    flex: 1 1 auto;
    min-width: 9ch;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .header-right {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  /* Detalhe do estado ("Coalescing… 44s"): vive sob o nome, no lugar de onde o chip esta. */
  .header-detail {
    margin: 2px 0 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.3;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Nome da sessao vira o titulo real do painel: no desktop largo a NavBar some e ele some junto. */
  .header-session {
    min-width: 0;
    overflow: hidden;
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    font-weight: var(--fw-semibold);
    line-height: 1.25;
    text-overflow: ellipsis;
    white-space: nowrap;
  }


  /* Faixa de acoes (ex-botoes da NavBar): icones sozinhos pediam memorizacao. Em 264px o par
     icone+rotulo cabe em duas colunas sem virar grade de cards. */
  /* Barra de acoes: uma linha so, um bloco por acao (icone em cima, rotulo curto embaixo), dentro
     de uma unica superficie — le como toolbar do painel, nao como quatro cards. Quantas couberem
     (o Atividade so existe as vezes): auto-fit divide a linha por igual. */
  /* Flex, não grid de colunas iguais: o divisor antes do Rodar é um item de 1px, e num
     `repeat(auto-fit, 1fr)` ele ganharia a largura de um botão. */
  /* A lista é configurável e sem teto: quando não cabe, a fileira ROLA na horizontal (decisão da
     sessão de grilling — sem limite artificial, sem menu "mais"). Com poucas, cada botão cresce e
     divide a linha como antes. */
  .ctx-actions {
    display: flex;
    align-items: stretch;
    gap: 2px;
    margin: 0 var(--space-4) var(--space-3);
    padding: 2px;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--surface-inset);
    overflow-x: auto;
    scrollbar-width: thin;
  }
  /* min-width é o PISO: poucas ações dividem a linha por igual (ellipsis no rótulo, como sempre);
     muitas param de encolher no piso e a fileira rola. */
  .ctx-actions > .ctx-action { flex: 1 1 0; min-width: 44px; }
  .acao-divisor {
    flex: 0 0 1px;
    align-self: center;
    height: 22px;
    margin: 0 var(--space-1);
    background: var(--border-default);
  }

  .ctx-action {
    position: relative;
    min-width: 0;
    min-height: 50px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
    padding: 6px 2px 5px;
    border: 0;
    border-radius: calc(var(--radius-md) - 3px);
    background: transparent;
    color: var(--text-secondary);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.01em;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }
  .ctx-action:hover { background: var(--surface-raised); color: var(--text-primary); }
  .ctx-action:disabled { opacity: 0.45; cursor: default; background: transparent; }
  .ctx-action:active { background: var(--bg-hover); }
  .ctx-action:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .ctx-action svg { flex-shrink: 0; width: 18px; height: 18px; }
  .ctx-action span {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1;
  }

  .animated-icon { display: inline-flex; flex-shrink: 0; }

  .terminal-btn { color: var(--text-secondary); }
  .terminal-btn.alert { color: var(--accent); }
  .terminal-btn.alert .animated-icon { animation: breathe 1.4s ease-in-out infinite; }

  .run-btn { position: relative; }
  .run-btn.running { color: var(--success); }
  .run-btn.running::after {
    content: ''; position: absolute; top: 6px; right: 6px; width: 6px; height: 6px;
    border-radius: 50%; background: var(--success); animation: pulse-scale 1.6s var(--ease-out) infinite;
  }

  @keyframes breathe {
    0%, 100% { opacity: 0.55; transform: scale(0.92); }
    50%      { opacity: 1;    transform: scale(1.05); }
  }
  @media (prefers-reduced-motion: reduce) {
    .terminal-btn.alert .animated-icon { animation: none; }
  }

  /* Turno ativo: hairline accent varrendo o TOPO do painel (a irma da work-sweep da NavBar).
     prefers-reduced-motion global (app.css) ja neutraliza o loop. */
  .ctx-sweep {
    position: sticky;
    top: 0;
    height: 2px;
    z-index: 1;
    overflow: hidden;
  }
  /* Anima transform, nao background-position: o compositor faz sozinho, sem repintar o painel
     a cada quadro. Faixa de 50% indo de -60% a 160% de si mesma = o mesmo percurso de antes. */
  .ctx-sweep::after {
    content: '';
    position: absolute;
    inset: 0;
    width: 50%;
    background: linear-gradient(90deg, transparent, var(--accent), transparent);
    animation: ctx-sweep 1.8s ease-in-out infinite;
  }
  @keyframes ctx-sweep {
    0%   { transform: translateX(-60%); }
    100% { transform: translateX(160%); }
  }

  /* Barra de abas Contexto | Arquivos (desenho do mock aprovado). O padding lateral segue o
     resto do painel (--space-4), nao os 10px do mock — a aba nasce dentro de um card que ja
     alinha tudo em 16px, e 10px desalinharia com o header e as secoes (divergencia mock-vs-app
     resolvida a favor do app). */
  .abas {
    flex: 0 0 auto;
    display: flex;
    gap: 2px;
    padding: 0 var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
  }
  .aba {
    appearance: none;
    background: none;
    border: 0;
    border-bottom: 2px solid transparent;
    color: var(--text-muted);
    font: inherit;
    font-size: var(--text-xs);
    padding: 9px 10px 8px;
    cursor: pointer;
    margin-bottom: -1px;
    /* Sobrescreve o alvo global de 44px (app.css) — a barra de abas e compacta por desenho
       (mock: ~34px de altura). */
    min-height: 0;
    min-width: 0;
  }
  .aba.sel {
    color: var(--text-primary);
    border-bottom-color: var(--accent);
  }
  .aba:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }

  /* Ritmo: seis secoes com o MESMO padding e uma hairline cada viravam uma escada monotona. As
     medidas (contexto, limites) andam juntas e sem regua entre elas; a regua so aparece onde o
     assunto muda de fato — medidas | grupo | ambiente. */
  section {
    margin: 0 var(--space-4);
    padding: var(--space-4) 0 var(--space-3);
  }

  /* O topo vivo: mesma sangria das seções, mais respiro embaixo — é ele que separa "o que está
     acontecendo" da lista de portas que vem depois. */
  .sec-agora { padding-bottom: var(--space-4); }
  .agora-topo {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: var(--space-3);
    margin-bottom: var(--space-2);
  }
  .agora-ctx { display: flex; align-items: baseline; gap: var(--space-2); min-width: 0; }
  .agora-num {
    display: inline;
    flex-shrink: 0;
    color: var(--text-primary);
    font-size: 44px;
    font-weight: var(--fw-semibold);
    line-height: 1;
    letter-spacing: -0.02em;
    /* O percentual redesenha a cada turno — sem tabular o número dança na largura. */
    font-variant-numeric: tabular-nums;
  }
  .agora-num.tone-warn { color: var(--warning); }
  .agora-num.tone-hot { color: var(--error); }
  /* Sem medição o traço não pode ter o peso de um número real. */
  .agora-num.vazio { color: var(--text-muted); }
  .agora-cap {
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--text-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .agora-custo { display: flex; flex-direction: column; align-items: flex-end; flex-shrink: 0; }
  .agora-custo strong {
    display: inline;
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: var(--fw-semibold);
    font-variant-numeric: tabular-nums;
  }
  .agora-custo span { color: var(--text-muted); font-size: var(--text-2xs); }
  .agora-linha {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-2);
    margin-top: var(--space-2);
    font-size: var(--text-2xs);
  }
  .agora-linha span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  @container (max-width: 380px) {
    .ctx-actions { flex-wrap: wrap; }
    .ctx-actions > .ctx-action { flex: 1 1 calc(50% - 2px); }
    .acao-divisor { display: none; }

    .agora-topo {
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      align-items: start;
      column-gap: var(--space-2);
      row-gap: 2px;
    }
    .agora-ctx { display: contents; }
    .agora-num { grid-column: 1; grid-row: 1; overflow: visible; }
    .agora-custo { grid-column: 2; grid-row: 1; }
    .agora-cap {
      grid-column: 1 / -1;
      grid-row: 2;
      overflow: visible;
      white-space: normal;
    }
    .agora-linha { display: block; }
    .agora-linha span {
      display: block;
      overflow: visible;
      white-space: normal;
    }
    .agora-linha span + span { margin-top: 2px; }

    .ctx-aviso { flex-wrap: wrap; }
    .ctx-aviso-texto { flex-basis: 100%; }
    .ctx-aviso-btn { margin-left: auto; }
  }

  /* Limites dentro do topo vivo: respiro entre a barra de contexto e as de cota, sem régua
     (continuam sendo o mesmo assunto). */
  .saude-limites { margin-top: var(--space-4); }
  /* 5h e 7d LADO A LADO: empilhadas gastavam quatro linhas do bloco mais importante do painel.
     `auto-fit` devolve o empilhamento sozinho quando a pessoa estreita a coluna. O override é
     local (o RateChips serve outras telas, onde a coluna é a do celular). */
  .saude-limites :global(.bars) {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(118px, 1fr));
    gap: var(--space-3);
  }
  .sec-break {
    padding-top: var(--space-4);
    border-top: 1px solid var(--border-subtle);
  }

  .section-label {
    display: block;
    margin-bottom: var(--space-2);
    color: var(--text-muted);
    /* Receita unificada de rotulo de secao (tokens de app.css) — era 10px/700/0.08em so aqui. */
    font-size: var(--label-size);
    font-weight: var(--label-weight);
    letter-spacing: var(--label-tracking);
    text-transform: uppercase;
  }


  /* Chip do loop (🔁 N/M): mono como os badges numericos; cor vem do tone via style inline. */
  .loop-chip {
    margin-left: var(--space-1);
    font-family: var(--font-mono); font-size: var(--text-xs); font-weight: 600;
    padding: 2px 8px; border-radius: var(--radius-full);
    background: var(--surface-card); border: 1px solid var(--border-subtle); cursor: pointer;
  }
  .loop-chip:active { background: var(--bg-hover); }

  /* Provider tappavel (Codex -> limites): visual do <strong> irmao, comportamento de botao. */
  .provider-tap {
    display: block;
    width: 100%;
    padding: 0;
    text-align: left;
    overflow: hidden;
    color: var(--accent);
    font-size: var(--text-xs);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-radius: var(--radius-sm);
  }

  section strong {
    display: block;
    overflow: hidden;
    color: var(--text-secondary);
    font-size: var(--text-xs);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  p {
    margin: 3px 0 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.45;
  }

  .mono { font-family: var(--font-mono); }

  /* Faixa de aviso acionável: âmbar de atenção, não de erro — nada quebrou, só está velho. */
  .ctx-aviso {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    margin: 0 var(--space-4) var(--space-2);
    padding: var(--space-2) var(--space-3);
    border: 1px solid color-mix(in srgb, var(--warning) 32%, transparent);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--warning) 10%, transparent);
  }
  .ctx-aviso-texto {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    color: var(--text-secondary);
    font-size: var(--text-2xs);
    line-height: 1.35;
  }
  .ctx-aviso-texto svg { flex-shrink: 0; color: var(--warning); }
  .ctx-aviso-btn {
    flex-shrink: 0;
    min-height: 0;
    padding: 4px 10px;
    border: 1px solid color-mix(in srgb, var(--warning) 45%, transparent);
    border-radius: var(--radius-full);
    color: var(--warning);
    font-size: var(--text-2xs);
    font-weight: var(--fw-semibold);
  }
  .ctx-aviso-btn:disabled { opacity: 0.5; cursor: default; }
  .ctx-aviso-acoes { display: flex; align-items: center; gap: var(--space-2); flex-shrink: 0; }
  /* Passou de 85%: a compactação automática do agente está perto, e o aviso deixa de ser
     "quando puder" pra ser "agora". */
  .ctx-aviso.grave {
    border-color: color-mix(in srgb, var(--error) 34%, transparent);
    background: color-mix(in srgb, var(--error) 10%, transparent);
  }
  .ctx-aviso.grave .ctx-aviso-texto svg { color: var(--error); }
  .ctx-aviso.grave .ctx-aviso-btn {
    color: var(--error);
    border-color: color-mix(in srgb, var(--error) 45%, transparent);
  }

  /* Diff do working tree: verde/vermelho são os mesmos do resto do app (tokens de estado), e o
     contador de arquivos fica muted — ele é o contexto dos dois números, não um terceiro. */
  .repo-diff { display: flex; align-items: baseline; gap: var(--space-2); }
  .diff-add { color: var(--success); font-variant-numeric: tabular-nums; }
  .diff-del { color: var(--error); font-variant-numeric: tabular-nums; }
  .diff-files { color: var(--text-muted); }

  .arq-topo {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-2);
    margin-top: var(--space-3);
  }
  .arq-titulo {
    color: var(--text-muted);
    font-size: var(--label-size);
    font-weight: var(--label-weight);
    letter-spacing: var(--label-tracking);
    text-transform: uppercase;
  }
  .arq-todos {
    min-height: 0;
    padding: 0;
    color: var(--text-muted);
    font-size: var(--text-2xs);
  }
  .arq-todos:hover { color: var(--text-secondary); }
  .arq-lista {
    margin: var(--space-2) 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .arq-lista li { display: flex; }
  /* Linha inteira clicável (abre no editor), com o realce sangrando pras bordas do painel — mesma
     receita do .sec-open, pra não virar um card dentro do card. */
  .arq-linha {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: var(--space-2);
    width: calc(100% + var(--space-4));
    min-height: 0;
    margin: 0 calc(var(--space-2) * -1);
    padding: 3px var(--space-2);
    border-radius: var(--radius-sm);
    text-align: left;
    transition: background 160ms var(--ease-out);
  }
  .arq-linha:hover:not(:disabled) { background: var(--bg-hover); }
  .arq-linha:disabled { cursor: default; }
  .arq-linha .diff-add { margin-left: auto; }
  .arq-path {
    min-width: 0;
    overflow: hidden;
    color: var(--text-secondary);
    font-size: var(--text-2xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .arq-lista .diff-add { flex-shrink: 0; font-size: var(--text-2xs); }

  /* Rodapé ancorado (fora do scroller): o que não muda na vida da sessão. */
  .ctx-rodape {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    flex-shrink: 0;
    padding: var(--space-3) var(--space-4);
    border-top: 1px solid var(--border-subtle);
  }
  .rodape-exec {
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--text-2xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ctx-rodape .provider-tap { font-size: var(--text-2xs); }
  .rodape-fila {
    flex-shrink: 0;
    color: var(--text-secondary);
    font-size: var(--text-2xs);
    font-variant-numeric: tabular-nums;
  }

  /* Secao que ABRE alguma coisa: o conteudo fica igual ao das secoes mudas (mesma tipografia, mesmo
     alinhamento) e o que muda e o alvo inteiro ficar clicavel, com um chevron discreto na direita.
     Sem caixa nem borda: virariam cards aninhados dentro do painel. */
  .sec-open {
    display: flex;
    min-height: 0;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    /* Sangra os --space-2 laterais pra fora da secao: o texto continua alinhado com o rotulo, e o
       realce do hover cobre a largura util do painel em vez de uma faixa recuada. */
    margin: calc(var(--space-1) * -1) calc(var(--space-2) * -1);
    padding: var(--space-1) var(--space-2);
    width: calc(100% + var(--space-4));
    border-radius: var(--radius-md);
    text-align: left;
    transition: background 160ms var(--ease-out);
  }
  .sec-open:hover { background: var(--bg-hover); }
  .sec-open:hover .sec-open-arrow { color: var(--text-secondary); transform: translateX(2px); }
  .sec-open-body { min-width: 0; }
  /* Acao secundaria da secao: peso de link, nao de botao — quem manda na linha e o atalho acima. */
  .sec-side {
    min-height: 0;
    min-width: 0;
    margin-top: var(--space-1);
    padding: 2px 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-color: var(--border-default);
  }
  .sec-side:hover { color: var(--text-secondary); }
  .sec-open-arrow {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--text-base);
    line-height: 1;
    transition: color 160ms var(--ease-out), transform 160ms var(--ease-out);
  }

  .progress {
    height: 4px;
    margin-top: var(--space-3);
    overflow: hidden;
    border-radius: var(--radius-full);
    background: var(--surface-raised);
  }

  .progress span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width 400ms var(--ease-out), background 300ms ease;
  }
  .progress.tone-warn span { background: var(--warning); }
  .progress.tone-hot span { background: var(--error); }

  .turn-tokens {
    margin-top: var(--space-2);
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--text-2xs);
    font-variant-numeric: tabular-nums;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Botão de recolher: mesmo ícone, tamanho e peso do irmão na barra esquerda (.fold-btn) — os dois
     fazem a mesma coisa em lados opostos. O cabeçalho reserva a faixa dele (padding-right acima),
     senão ele cai por cima do chip de estado: absoluto não empurra nada. */
  .ctx-fold {
    position: absolute; top: var(--space-2); right: var(--space-2); z-index: 2;
    width: 36px; height: 36px; display: grid; place-items: center;
    background: transparent; border: 0; border-radius: var(--radius-md);
    color: var(--text-secondary); cursor: pointer;
  }
  @media (hover: hover) { .ctx-fold:hover { background: var(--bg-hover); color: var(--text-primary); } }

  /* RECOLHIDO = escondido, não trilho nem aba flutuante. Clarificação final do usuário: a aba
     vertical isolada no MEIO da borda direita (26×64, top:50%) morreu nos DOIS modos. Com toggle
     externo (barra no modo tabs OU rodapé do rail), o painel SOME de vez (display:none). Sem
     toggle externo (sidebar expandida), sobra uma PORTA DISCRETA no topo — o ctx-fold na posição
     do header (36px), não uma aba flutuante: clicar reexpande. */
  .session-context.recolhido {
    /* Trilho: largura fixa e sem folga nenhuma — a aba encosta na borda, que é o que faz ela ler
       como puxador. A coluna do shell reserva exatamente estes 36px (LARGURA_TRILHO), então
       qualquer margem aqui viraria estouro e sobraria faixa vazia ao lado. */
    flex: none;
    width: 36px;
    margin: 0;
    border-color: transparent;
    box-shadow: none;
    pointer-events: none;
  }
  .session-context.recolhido::before { opacity: 0; }
  .session-context.recolhido .ctx-fold {
    pointer-events: auto;
    position: static;
    width: 36px; height: 36px;
    margin: var(--space-2) auto 0;
    border-radius: var(--radius-md);
    background: var(--surface-raised);
    box-shadow: none;
  }
  .session-context.recolhido.toggle-externo {
    display: none;
    width: 0;
  }

  @media (max-width: 1279px) {
    .session-context { display: none; }
  }
</style>

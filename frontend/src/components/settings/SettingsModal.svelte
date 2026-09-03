<script lang="ts">
  import BottomSheet from '../BottomSheet.svelte';
  import SettingsRow from './SettingsRow.svelte';
  import GeneralSettings from './GeneralSettings.svelte';
  import AppearanceSettings from './AppearanceSettings.svelte';
  import DictationSettings from './DictationSettings.svelte';
  import VozSettings from './VozSettings.svelte';
  import ServerSettings from './ServerSettings.svelte';
  import EnginesSettings from './EnginesSettings.svelte';
  import SobreSettings from './SobreSettings.svelte';
  import DiarioSettings from './DiarioSettings.svelte';
  import ServidoresSettings from './ServidoresSettings.svelte';
  import AcessoSettings from './AcessoSettings.svelte';
  import ContasSettings from './ContasSettings.svelte';
  import ServidorSeletor from './ServidorSeletor.svelte';
  import ConfigIcone from './ConfigIcone.svelte';
  import { criarConfigServidor } from '../../lib/serverConfig.svelte';
  import { TELAS_DE_SERVIDOR, type TelaConfig } from '../../lib/configRoute';
  import { fly, fade } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import OrquestracaoContas from '../OrquestracaoContas.svelte';
  import * as m from '../../paraglide/messages';
  import { listServers, onServersChanged, type Server } from '../../lib/auth';

  interface Props {
    tela: TelaConfig;
    alvo: Server | null;
    nomeAlvo: string | null;
    semServidor: boolean;
    // Identidade COMPOSTA (JSON de id+label+baseUrl+token) do servidor sendo configurado. O store e
    // o $effect observam ISTO, não `alvo.id`: trocar base/token com o mesmo id (rotação de token,
    // re-parear) é outra entidade — inicia load novo em vez de achar que é o mesmo alvo. Resolvida
    // no App inclusive no modo global (aí é o servidor ATIVO, já que `alvo` fica null).
    identidade: string;
    // Servidor RESOLVIDO (?srv=) + porta de escolha de alvo + logout global — só a tela
    // Servidores usa; o resto continua com `alvo` (apiTarget).
    resolvedServer?: Server | null;
    onPickServer?: (id: string) => void;
    onLogout?: () => void | Promise<void>;
    onIrPara: (t: TelaConfig) => void;
    onVoltar: () => void;
    onFechar: () => void;
  }
  let { tela, alvo, nomeAlvo, semServidor, identidade, resolvedServer = null, onPickServer, onLogout, onIrPara, onVoltar, onFechar }: Props = $props();

  const store = criarConfigServidor(() => alvo, () => identidade);

  // Seletor de servidor do grupo "Servidor" (pedido repetido do usuário, 19/08/2026): o rótulo
  // "Servidor · X" dizia o alvo mas não trocava — trocar exigia ir à tela Servidores e voltar.
  // listServers() lê localStorage e não é reativo; o contador sobe pelo mesmo onServersChanged
  // que o App e o ServidoresSettings usam.
  let versaoServidores = $state(0);
  $effect(() => onServersChanged(() => versaoServidores++));
  const servidores = $derived.by(() => {
    versaoServidores;
    return listServers();
  });
  // O alvo corrente é o resolvedServer: o App já resolve pro ATIVO quando não há ?srv=
  // (App.svelte, alvoConfig), então null aqui é só "alvo que não resolveu" — e nesse caso nem
  // tela de servidor abre (telaEfetiva cai na Aparencia).
  const servidorAtualId = $derived(resolvedServer?.id ?? '');
  // Com UM servidor não há escolha a fazer — o rótulo estático de sempre fica. Sem onPickServer
  // (quem monta sem a porta) também não faz sentido oferecer troca. E sem resolvedServer (um
  // ?srv= que não resolveu) o select abriria em BRANCO — nenhuma option casa com '' —, então a
  // condição exige o alvo resolvido (a tela em si já caiu na Aparencia nesse caso).
  const mostrarSeletor = $derived(servidores.length > 1 && !!onPickServer && !!resolvedServer);
  function trocarServidor(id: string) { onPickServer?.(id); }

  // Na tela Servidores o store fica EM SILENCIO (zero GET /api/config) e a operacao pendente é
  // INVALIDADA sem nova chamada: quem manda la sao os controllers proprios (ServerManager/
  // PushQuiet). O store SÓ carrega quando (a) a IDENTIDADE mudou (troca real de alvo: outro
  // servidor, ou o mesmo id com base/token/label diferentes — limpa o estado do alvo anterior) ou
  // (b) se voltou de Servidores pro alvo corrente — trocar de tela no MESMO alvo preserva o
  // rascunho único (decisão do usuário; o store decide pelo ultimoDono, não pela tela).
  let identidadeAnterior = $state<string | undefined>(undefined);
  // null no primeiro run: `veioDeServidores` exige telaAnterior === 'servidores', então o boot
  // cai no ramo `mudouAlvo` (carrega) — comportamento idêntico, sem capturar `tela` no $state.
  let telaAnterior = $state<TelaConfig | null>(null);
  $effect(() => {
    const id = identidade;
    const mudouAlvo = id !== identidadeAnterior;
    const veioDeServidores = telaAnterior === 'servidores' && tela !== 'servidores';
    identidadeAnterior = id;
    telaAnterior = tela;
    if (tela === 'servidores') { store.invalidar(); return; }
    if (mudouAlvo) store.carregar();
    else if (veioDeServidores) store.carregar();
  });

  const TITULO: Record<TelaConfig, string> = {
    root: m.config_modal_titulo(),
    geral: m.config_geral_titulo(),
    aparencia: m.config_modal_aparencia(),
    ditado: m.config_modal_ditado(),
    voz: m.voz_titulo(),
    sobre: m.config_modal_sobre(),
    diario: m.config_diag_titulo(),
    servidores: m.config_modal_servidores(),
    acesso: m.acesso_titulo(),
    contas: m.contas_titulo(),
    notificacoes: m.config_modal_notificacoes(),
    anexos: m.config_modal_anexos(),
    avancado: m.config_modal_avancado(),
    motores: m.config_modal_motores(),
    orquestracao: m.config_modal_orquestracao(),
  };

  // Valores de rotulo vindo de funcao (m.*) dependem do locale: o `as const` nao pode mais
  // existir (valor de funcao nao e literal), mas o `satisfies` fica — e ele que checa a forma.
  const LINHAS = [
    { id: 'geral', secao: 'app', rotulo: m.config_geral_linha(), icone: 'globo', servidor: false },
    { id: 'aparencia', secao: 'app', rotulo: m.config_modal_aparencia(), icone: 'pincel', servidor: false },
    { id: 'diario', secao: 'app', rotulo: m.config_diag_titulo(), icone: 'recibo', servidor: false },
    { id: 'sobre', secao: 'app', rotulo: m.config_modal_sobre(), icone: 'info', servidor: false },
    { id: 'acesso', secao: 'servidor', rotulo: m.acesso_titulo(), icone: 'sinal', servidor: true },
    { id: 'contas', secao: 'servidor', rotulo: m.contas_titulo(), icone: 'pessoa', servidor: true },
    { id: 'servidores', secao: 'servidor', rotulo: m.config_modal_servidores(), icone: 'tela', servidor: false },
    { id: 'voz', secao: 'servidor', rotulo: m.voz_titulo(), icone: 'mic', servidor: true },
    { id: 'notificacoes', secao: 'servidor', rotulo: m.config_modal_notificacoes(), icone: 'sino', servidor: true },
    { id: 'anexos', secao: 'servidor', rotulo: m.config_modal_anexos(), icone: 'clipe', servidor: true },
    { id: 'avancado', secao: 'servidor', rotulo: m.config_modal_avancado(), icone: 'chave', servidor: true },
    { id: 'motores', secao: 'servidor', rotulo: m.config_modal_motores(), icone: 'plug', servidor: true },
    { id: 'orquestracao', secao: 'servidor', rotulo: m.config_modal_orquestracao(), icone: 'sliders', servidor: true },
  ] satisfies readonly { id: TelaConfig; secao: string; rotulo: string; icone: string; servidor: boolean }[];
  const SECOES = ['app', 'servidor'] as const;

  // Troca de tela do modal: fly curto na direção da navegação (180ms ease-out, uso ocasional).
  // Entrar numa sub-tela vem da direita; voltar pra raiz vem da esquerda. Com reduced-motion
  // vira só fade (opacidade não é movimento — regra de acessibilidade da Emil). Com LISTENER,
  // não snapshot: quem alterna a preferência do SO com o modal aberto não fica com fly vivo.
  let semMovimento = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const on = () => (semMovimento = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });
  function animarTela(node: HTMLElement, { x }: { x: number }) {
    if (semMovimento) return fade(node, { duration: 120 });
    return fly(node, { x, duration: 180, easing: cubicOut });
  }

  let tituloEl = $state<HTMLElement | null>(null);
  // Fallback de foco das confirmações da tela Servidores: o botão FECHAR do modal é o controle que
  // sempre sobra acessível. Desktop e mobile têm botões diferentes — ambos fazem bind no MESMO ref.
  let fecharEl = $state<HTMLElement | null>(null);

  // Trocar de rota destroi a linha que tinha o foco e o activeElement cai no <body>: leitor de tela
  // fica mudo e o Tab recomeca do zero. Mover o foco pro titulo (que muda a cada tela) anuncia a
  // troca e da um ponto de partida.
  $effect(() => { tela; tituloEl?.focus(); });

  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });

  // No desktop a raiz (a lista de linhas) so existe no celular: as duas colunas ja mostram a
  // navegacao inteira, entao 'root' cai na Aparencia. Resolvido NO RENDER, nunca por navegacao —
  // um redirect disparado no resize seria um push a mais no historico, que a rota ja acerta sozinha.
  const telaAtual = $derived(isDesktop && tela === 'root' ? ('aparencia' as TelaConfig) : tela);

  // VOLTAR e FECHAR sao coisas diferentes, e o BottomSheet so conhece "fechar": ele chama onClose no
  // Esc, no toque no backdrop E no swipe pra baixo. Passar `voltar` como onClose faria o swipe — o
  // gesto mais usado no celular, que e a view deste plano — subir um nivel em vez de sair, e de
  // dentro de uma sub-tela a folha ficaria impossivel de fechar. Entao o BottomSheet recebe o
  // onFechar de verdade, e voltar e SO o botao do cabecalho.
  function botaoEsquerdo() {
    if (tela === 'root') onFechar();
    else onVoltar();
  }

  // VER AO VIVO (so desktop, so na Aparencia): o painel grande sai da frente e vira uma caixinha
  // flutuante no canto, sem veu, com o chat REAL atras — porque todo slider daqui edita justamente o
  // que o painel esta tapando. A previa embutida resolve o caso comum; isto e pra quando a pessoa
  // quer ver a conversa de verdade mudando.
  let aoVivo = $state(false);
  // Sair da Aparencia (ou ir pro celular) desliga: a caixinha e pequena demais pra tela de motores, e
  // no celular nao ha "atras" pra revelar.
  $effect(() => { if (telaAtual !== 'aparencia' || !isDesktop) aoVivo = false; });

  // ── Arrastar a caixinha pelo cabecalho ────────────────────────────────────────────────────────
  // Mesmo desenho do Canvas (dragStart/dragMove/dragEnd com setPointerCapture): a faixa de arrasto e
  // so o cabecalho, porque o corpo e cheio de slider — arrastar por ele viraria briga com o controle.
  const POS_KEY = 'cp_aparencia_vivo_pos';
  let pos = $state<{ x: number; y: number } | null>(lerPos());
  function lerPos(): { x: number; y: number } | null {
    try {
      const cru = localStorage.getItem(POS_KEY);
      if (!cru) return null;
      const p = JSON.parse(cru);
      if (typeof p?.x !== 'number' || typeof p?.y !== 'number') return null;
      return dentroDaTela(p);
    } catch { return null; }
  }
  // Prende na janela ATUAL. A posicao guardada pode ter nascido numa tela maior (monitor externo,
  // janela maximizada): sem isto a caixinha renderizaria fora da viewport levando junto o cabecalho,
  // que e o unico lugar com o ✕ — e como ela nao tem backdrop nem fecha no Esc, a saida seria apagar
  // a chave do localStorage na mao. Vale na leitura E no resize, nao so enquanto se arrasta.
  function dentroDaTela(p: { x: number; y: number }): { x: number; y: number } {
    return {
      x: Math.min(Math.max(0, p.x), Math.max(0, window.innerWidth - 120)),
      y: Math.min(Math.max(0, p.y), Math.max(0, window.innerHeight - 40)),
    };
  }
  $effect(() => {
    const on = () => { if (pos) pos = dentroDaTela(pos); };
    window.addEventListener('resize', on);
    return () => window.removeEventListener('resize', on);
  });
  let drag: { dx: number; dy: number } | null = null;
  let caixaEl = $state<HTMLElement | null>(null);
  function dragStart(e: PointerEvent) {
    // Os botoes (✕ e voltar-ao-painel) vivem DENTRO da faixa de arrasto. Sem esta saida, o
    // setPointerCapture + preventDefault abaixo engoliam o clique deles e a caixinha ficava
    // impossivel de fechar — so o cursor de mover aparecia.
    if ((e.target as HTMLElement).closest('button')) return;
    if (!caixaEl) return;
    const r = caixaEl.getBoundingClientRect();
    drag = { dx: e.clientX - r.left, dy: e.clientY - r.top };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function dragMove(e: PointerEvent) {
    if (!drag || !caixaEl) return;
    const r = caixaEl.getBoundingClientRect();
    // Prende dentro da janela: solto num canto fora da tela, a caixinha so sairia dali por
    // localStorage na mao. Deixa 40px de cabecalho sempre alcancavel.
    const x = Math.min(Math.max(0, e.clientX - drag.dx), window.innerWidth - Math.min(r.width, 120));
    const y = Math.min(Math.max(0, e.clientY - drag.dy), window.innerHeight - 40);
    pos = { x, y };
  }
  function dragEnd() {
    if (!drag) return;
    drag = null;
    try { if (pos) localStorage.setItem(POS_KEY, JSON.stringify(pos)); } catch { /* modo privado */ }
  }
</script>

{#if aoVivo}
  <!-- Caixinha: `fixed` no canto, SEM backdrop — o clique fora vai pro app, que e o ponto. Fecha so
       no ✕ (ou voltando pro painel), como pedido. Nao usa BottomSheet: aquele e um dialogo modal com
       veu e trap de foco, o oposto do que se quer aqui. -->
  <!-- `role="region"`, nao `dialog`: dialogo promete semantica modal (foco presto, Esc fecha) e esta
       caixinha e o oposto de proposito — o app atras segue clicavel e ela so sai no ✕. -->
  <div class="vivo" role="region" aria-label={m.config_modal_ao_vivo_aria()} bind:this={caixaEl}
       style={pos ? `left: ${pos.x}px; top: ${pos.y}px; right: auto; bottom: auto;` : ''}>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header class="vivo-head" onpointerdown={dragStart} onpointermove={dragMove}
            onpointerup={dragEnd} onpointercancel={dragEnd}>
      <h2 class="vivo-titulo">{m.config_modal_aparencia()}</h2>
      <button class="vivo-btn" onclick={() => (aoVivo = false)} title={m.config_modal_voltar_painel()}>⤢</button>
      <button class="vivo-btn" onclick={onFechar} aria-label={m.sessao_fechar()}>✕</button>
    </header>
    <div class="vivo-corpo">
      <AppearanceSettings semPrevia />
    </div>
  </div>
{:else}
<BottomSheet open={true} onClose={onFechar} ariaLabel={TITULO[telaAtual]}
             wide={isDesktop} centered={isDesktop} split={isDesktop}>
  {#if isDesktop}
    <div class="st-split">
      <!-- O ✕ fica FORA da coluna que rola: dentro dela, `position: absolute` rolaria junto com o
           conteudo (e `sticky` num container com padding brigaria com o topo do formulario).
           Primeiro no DOM (ele e `position: absolute`, a ordem aqui nao muda onde ele aparece) pra
           quem navega por teclado chegar nele antes de percorrer a navegacao e o conteudo inteiros. -->
      <button class="st-fechar" bind:this={fecharEl} onclick={onFechar} aria-label={m.sessao_fechar()}>✕</button>
      <aside class="st-nav">
        {#each SECOES as secao (secao)}
          {#if secao === 'servidor' && mostrarSeletor}
            <!-- O rótulo do grupo vira o TROCADOR de alvo: "Servidor" + select com a máquina
                 sendo configurada. Antes dizia "Servidor · X" e trocar exigia ir à tela
                 Servidores e voltar (pedido recorrente do usuário). -->
            <div class="st-secao st-secao-sel">
              <span>{m.lista_agrupar_servidor()}</span>
              <ServidorSeletor {servidores} atualId={servidorAtualId} onTrocar={trocarServidor} />
            </div>
          {:else}
            <p class="st-secao">{secao === 'servidor' && nomeAlvo ? m.config_modal_servidor_de({ nome: nomeAlvo }) : secao === 'servidor' ? m.lista_agrupar_servidor() : m.config_aparencia_app()}</p>
          {/if}
          {#each LINHAS.filter((l) => l.secao === secao) as l (l.id)}
            <button class="st-nav-item" class:sel={telaAtual === l.id}
                    aria-current={telaAtual === l.id ? 'page' : undefined}
                    disabled={l.servidor && semServidor}
                    onclick={() => onIrPara(l.id)}>
              <span class="st-nav-icone" aria-hidden="true"><ConfigIcone nome={l.icone} size={16} /></span>{l.rotulo}
            </button>
          {/each}
        {/each}
      </aside>
      <section class="st-conteudo">
        <!-- #key por tela: a troca remonta o conteúdo, e o wrapper novo entra voando (a saída do
             antigo é instantânea de propósito — transição de saída aqui brigaria com a nova). -->
        {#key telaAtual}<div in:animarTela={{ x: 18 }}>{@render corpo()}</div>{/key}
      </section>
    </div>
  {:else}
    <header class="st-head">
      <button class="st-icone" bind:this={fecharEl} onclick={botaoEsquerdo}
        aria-label={tela === 'root' ? m.sessao_fechar() : m.comum_voltar()}>{tela === 'root' ? '✕' : '‹'}</button>
      <!-- tabindex=-1: alvo do foco na troca de tela, sem entrar na ordem do Tab. -->
      <h2 class="st-titulo" bind:this={tituloEl} tabindex="-1">{TITULO[tela]}</h2>
      <span class="st-icone st-vazio" aria-hidden="true"></span>
      {#if TELAS_DE_SERVIDOR.includes(telaAtual) && nomeAlvo}
        <!-- Sem isto nao da pra saber em que maquina se esta mexendo: o app roda no front de um
             servidor e a lista e agregada, entao a config aberta pode ser de outra maquina. Com
             mais de um servidor o texto vira o TROCADOR (select) — dizer sem deixar trocar foi o
             pedido recorrente do usuário. -->
        <p class="st-sub">
          {#if mostrarSeletor}
            <ServidorSeletor {servidores} atualId={servidorAtualId} onTrocar={trocarServidor} />
          {:else}
            {m.config_modal_em({ nome: nomeAlvo })}
          {/if}
        </p>
      {/if}
    </header>
    {#key telaAtual}<div in:animarTela={{ x: telaAtual === 'root' ? -18 : 18 }}>{@render corpo()}</div>{/key}
  {/if}
</BottomSheet>
{/if}

{#snippet corpo()}
  {#if telaAtual === 'root'}
    {#each SECOES as secao (secao)}
      {#if secao === 'servidor' && mostrarSeletor}
        <div class="st-secao st-secao-sel">
          <span>{m.lista_agrupar_servidor()}</span>
          <ServidorSeletor {servidores} atualId={servidorAtualId} onTrocar={trocarServidor} />
        </div>
      {:else}
        <p class="st-secao">{secao === 'servidor' && nomeAlvo ? m.config_modal_servidor_de({ nome: nomeAlvo }) : secao === 'servidor' ? m.lista_agrupar_servidor() : m.config_aparencia_app()}</p>
      {/if}
      <div class="st-cartao">
        {#each LINHAS.filter((l) => l.secao === secao) as l (l.id)}
          <SettingsRow icone={l.icone} rotulo={l.rotulo}
            desabilitada={l.servidor && semServidor}
            motivo={m.config_modal_escolha_servidor()}
            onPick={() => onIrPara(l.id)} />
        {/each}
      </div>
    {/each}
  {:else if telaAtual === 'geral'}
    <GeneralSettings />
  {:else if telaAtual === 'aparencia'}
    <AppearanceSettings podeAoVivo={isDesktop} onVerAoVivo={() => (aoVivo = true)} />
  {:else if telaAtual === 'ditado'}
    <DictationSettings />
  {:else if telaAtual === 'motores'}
    <EnginesSettings targetServer={alvo} />
  {:else if telaAtual === 'orquestracao'}
    <OrquestracaoContas desktop={isDesktop} />
  {:else if telaAtual === 'diario'}
    <DiarioSettings />
  {:else if telaAtual === 'sobre'}
    <SobreSettings />
  {:else if telaAtual === 'servidores'}
    <ServidoresSettings resolvedServer={resolvedServer} apiTarget={alvo}
      fallbackFocus={fecharEl}
      onPickTarget={onPickServer ?? (() => {})} onLogout={onLogout ?? (() => {})} />
  {:else if telaAtual === 'acesso'}
    <!-- O alvo por PROP (não pela rota lida dentro da tela): com o seletor do grupo a troca
         não remonta a tela, e a prop reativa é o que remede os endereços (achado da revisão). -->
    <AcessoSettings alvo={resolvedServer} />
  {:else if telaAtual === 'contas'}
    <ContasSettings apiTarget={alvo} />
  {:else if telaAtual === 'voz'}
    <VozSettings {store} />
  {:else}
    <ServerSettings {store} secao={telaAtual} />
  {/if}
{/snippet}

<style>
  .st-head {
    display: grid; grid-template-columns: 32px 1fr 32px; align-items: center;
    gap: var(--space-2); margin-bottom: var(--space-4);
  }
  .st-titulo {
    margin: 0; text-align: center;
    font-size: var(--text-base); font-weight: 600; color: var(--text-primary);
  }
  .st-sub { grid-column: 2; margin: 0; text-align: center; font-size: var(--text-xs); color: var(--text-muted); }
  .st-titulo:focus { outline: none; }   /* alvo programatico: o anel aqui so confundiria */
  .st-icone {
    width: 32px; height: 32px; border-radius: var(--radius-full);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    color: var(--text-secondary); font-size: var(--text-base); line-height: 1; cursor: pointer;
  }
  .st-vazio { border: 0; background: transparent; }   /* espelha o botao pro titulo ficar no centro */
  .st-secao {
    margin: var(--space-4) 0 var(--space-1) var(--space-2);
    color: var(--text-muted);
    /* Receita unificada de rotulo de secao (tokens de app.css) — era 12px/400/0.05em so aqui. */
    font-size: var(--label-size);
    font-weight: var(--label-weight);
    text-transform: uppercase; letter-spacing: var(--label-tracking);
  }
  /* Versão com o seletor de alvo: o rótulo "Servidor" e o select na mesma linha. O select fica
     minúsculo de propósito — o nome da máquina é dado, não título de seção. */
  .st-secao-sel { display: flex; align-items: center; gap: var(--space-2); }
  /* Cartao arredondado com as linhas dentro, no formato do iOS. `--surface-card` entra no veu do
     papel de parede junto com o resto (CLAUDE.md, "Transparencia"). */
  .st-cartao {
    background: var(--surface-card);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .st-cartao > :global(button + button) { border-top: 1px solid var(--border-subtle); }

  /* Raiz no celular: linhas entrando em cascata ao abrir o modal (30ms entre elas, entrada só,
     nunca loop — a regra global de reduced-motion do app.css já cobre o resto). */
  @media (prefers-reduced-motion: no-preference) {
    .st-cartao > :global(button) { animation: st-row-in 240ms var(--ease-out) both; }
    .st-cartao > :global(button:nth-child(2)) { animation-delay: 30ms; }
    .st-cartao > :global(button:nth-child(3)) { animation-delay: 60ms; }
    .st-cartao > :global(button:nth-child(4)) { animation-delay: 90ms; }
    .st-cartao > :global(button:nth-child(5)) { animation-delay: 120ms; }
    .st-cartao > :global(button:nth-child(6)) { animation-delay: 150ms; }
    .st-cartao > :global(button:nth-child(7)) { animation-delay: 180ms; }
    .st-cartao > :global(button:nth-child(8)) { animation-delay: 210ms; }
  }
  @keyframes st-row-in {
    from { opacity: 0; transform: translateY(6px); }
    to   { opacity: 1; transform: translateY(0); }
  }

  /* ── Ao vivo: caixinha flutuante no canto direito ───────────────────────────────────────────
     Nao e um dialogo modal: sem backdrop, sem trap de foco, e o app atras continua clicavel. E o
     ponto da feature — mexer no slider e ver a conversa de verdade mudando. */
  .vivo {
    position: fixed;
    right: var(--space-4);
    bottom: var(--space-4);
    z-index: 90;                    /* acima do chat, abaixo dos dialogos de verdade (100+) */
    display: flex;
    flex-direction: column;
    width: min(360px, calc(100vw - var(--space-6)));
    max-height: min(70vh, 640px);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    /* Fundo PROPRIO e quase opaco, nao `--glass-panel`: esta caixinha pousa em cima da conversa e do
       painel de contexto, e com a alfa do painel (0,73) o texto de tras atravessava e embaralhava os
       rotulos dos sliders — medido na tela. Ela e chrome funcional flutuante, mesmo caso do card do
       menu da conta: precisa de leitura, nao de veu. O desfoque abaixo termina de separar. */
    background: var(--bg-surface);
    backdrop-filter: blur(18px) saturate(150%);
    /* --elev-3 no lugar da sombra a mao: mesmo desenho no escuro, e o claro ganha a versao leve. */
    box-shadow: var(--elev-3);
    overflow: hidden;
  }
  /* Faixa de arrasto: `cursor: move` e `touch-action: none` (sem o segundo, o navegador rola a pagina
     em vez de entregar o pointermove). Os botoes de dentro voltam ao cursor normal. */
  .vivo-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
    cursor: move;
    touch-action: none;
    user-select: none;
  }
  .vivo-head .vivo-btn { cursor: pointer; }
  .vivo-titulo {
    flex: 1;
    margin: 0;
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
  }
  .vivo-btn {
    width: 28px; height: 28px; min-height: 0; min-width: 0;
    border-radius: var(--radius-full);
    border: 1px solid var(--border-subtle);
    background: var(--surface-raised);
    color: var(--text-secondary);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  @media (hover: hover) { .vivo-btn:hover { color: var(--text-primary); background: var(--bg-hover); } }
  /* O corpo rola sozinho: a caixinha tem altura fixa e a Aparencia inteira nao cabe nela. */
  .vivo-corpo { overflow-y: auto; padding: var(--space-3) var(--space-4) var(--space-4); min-height: 0; }

  /* ⚠ `grid-template-rows: minmax(0, 1fr)` NAO e enfeite: sem ele a linha implicita e `auto`, a
     trilha cresce ate a altura do formulario, o overflow-y da coluna nunca dispara e o
     `overflow: hidden` do .sheet.split CORTA o fim do conteudo — inclusive o botao Salvar. */
  .st-split {
    display: grid;
    grid-template-columns: 232px 1fr;
    grid-template-rows: minmax(0, 1fr);
    height: 100%;
    position: relative;
  }
  .st-nav {
    overflow-y: auto; min-height: 0;
    padding: var(--space-3) var(--space-2);
    border-right: 1px solid var(--border-subtle);
  }
  .st-conteudo {
    overflow-y: auto; min-height: 0; padding: var(--space-4);
    /* O ✕ (.st-fechar) fica de fora do scroller, no canto superior direito de .st-split — sem
       reservar o espaço dele aqui em cima, ele flutua por cima do primeiro controle da tela (o
       segmentado de Tema, na Aparência). 44px é o tamanho REAL do botão: o `width`/`height: 32px`
       declarado nele perde pro tap-target mínimo global (`button { min-width/height: 44px }` em
       app.css) — medido no navegador, não os 32px do CSS. */
    padding-top: calc(var(--space-3) + 44px + var(--space-2));
  }
  .st-fechar {
    position: absolute; top: var(--space-3); right: var(--space-3); z-index: 1;
    width: 32px; height: 32px; border-radius: var(--radius-full);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    color: var(--text-secondary); font-size: var(--text-base); line-height: 1; cursor: pointer;
  }
  .st-nav-item {
    display: flex; align-items: center; justify-content: flex-start; gap: var(--space-2);
    width: 100%; padding: var(--space-2) var(--space-3); border: 0; border-radius: var(--radius-md);
    background: transparent; color: var(--text-primary); font-size: var(--text-sm); text-align: left;
    cursor: pointer;
  }
  @media (hover: hover) { .st-nav-item:not(:disabled):hover { background: var(--bg-hover); } }
  .st-nav-item.sel { background: var(--bg-elevated); }   /* realce de estado: --bg-* cru e o certo */
  .st-nav-item:disabled { color: var(--text-muted); cursor: default; }
  .st-nav-icone { flex-shrink: 0; width: 1.4em; display: grid; place-items: center;
                  color: var(--text-secondary); }
</style>

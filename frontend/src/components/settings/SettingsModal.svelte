<script lang="ts">
  import BottomSheet from '../BottomSheet.svelte';
  import SettingsRow from './SettingsRow.svelte';
  import AppearanceSettings from './AppearanceSettings.svelte';
  import ServerSettings from './ServerSettings.svelte';
  import EnginesSettings from './EnginesSettings.svelte';
  import { criarConfigServidor } from '../../lib/serverConfig.svelte';
  import { TELAS_DE_SERVIDOR, type TelaConfig } from '../../lib/configRoute';
  import type { Server } from '../../lib/auth';

  interface Props {
    tela: TelaConfig;
    alvo: Server | null;
    nomeAlvo: string | null;
    semServidor: boolean;
    onIrPara: (t: TelaConfig) => void;
    onVoltar: () => void;
    onFechar: () => void;
  }
  let { tela, alvo, nomeAlvo, semServidor, onIrPara, onVoltar, onFechar }: Props = $props();

  const store = criarConfigServidor(() => alvo);

  // Depender do ID, nao do objeto: `listServers()` faz JSON.parse a cada chamada (auth.ts), entao
  // `alvo` e um objeto NOVO a cada recomputo — um efeito que dependesse dele recarregaria (e
  // zeraria o rascunho) a cada troca de tela. O `?? '-'` cobre o caso sem alvo (servidor ativo).
  $effect(() => {
    alvo?.id ?? '-';
    store.carregar();
  });

  const TITULO: Record<TelaConfig, string> = {
    root: 'Configurações',
    aparencia: 'Aparência',
    notificacoes: 'Notificações',
    anexos: 'Anexos e transcrição',
    avancado: 'Avançado do servidor',
    motores: 'Motores de modelo',
  };

  const LINHAS = [
    { id: 'aparencia', secao: 'App', rotulo: 'Aparência', icone: '🎨',
      descricao: 'tema, fundo, leitura e texto', servidor: false },
    { id: 'notificacoes', secao: 'Servidor', rotulo: 'Notificações', icone: '🔔',
      descricao: 'quando avisar que terminou, caiu ou travou', servidor: true },
    { id: 'anexos', secao: 'Servidor', rotulo: 'Anexos e transcrição', icone: '📎',
      descricao: 'chave da Groq e por quanto tempo guardar', servidor: true },
    { id: 'avancado', secao: 'Servidor', rotulo: 'Avançado do servidor', icone: '🛠️',
      descricao: 'automações, editor e o que só muda pelo .env', servidor: true },
    { id: 'motores', secao: 'Servidor', rotulo: 'Motores de modelo', icone: '🔌',
      descricao: 'rodar uma sessão em outro provedor', servidor: true },
  ] as const satisfies readonly { id: TelaConfig; secao: string; rotulo: string; icone: string; descricao: string; servidor: boolean }[];
  const SECOES = ['App', 'Servidor'] as const;

  let tituloEl = $state<HTMLElement | null>(null);

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
      return typeof p?.x === 'number' && typeof p?.y === 'number' ? p : null;
    } catch { return null; }
  }
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
  <div class="vivo" role="dialog" aria-label="Aparência — ao vivo" bind:this={caixaEl}
       style={pos ? `left: ${pos.x}px; top: ${pos.y}px; right: auto; bottom: auto;` : ''}>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header class="vivo-head" onpointerdown={dragStart} onpointermove={dragMove}
            onpointerup={dragEnd} onpointercancel={dragEnd}>
      <h2 class="vivo-titulo">Aparência</h2>
      <button class="vivo-btn" onclick={() => (aoVivo = false)} title="Voltar ao painel">⤢</button>
      <button class="vivo-btn" onclick={onFechar} aria-label="Fechar">✕</button>
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
      <button class="st-fechar" onclick={onFechar} aria-label="Fechar">✕</button>
      <aside class="st-nav">
        {#each SECOES as secao (secao)}
          <p class="st-secao">{secao === 'Servidor' && nomeAlvo ? `Servidor · ${nomeAlvo}` : secao}</p>
          {#each LINHAS.filter((l) => l.secao === secao) as l (l.id)}
            <button class="st-nav-item" class:sel={telaAtual === l.id}
                    aria-current={telaAtual === l.id ? 'page' : undefined}
                    disabled={l.servidor && semServidor}
                    onclick={() => onIrPara(l.id)}>
              <span class="st-nav-icone" aria-hidden="true">{l.icone}</span>{l.rotulo}
            </button>
          {/each}
        {/each}
      </aside>
      <section class="st-conteudo">
        {@render corpo()}
      </section>
    </div>
  {:else}
    <header class="st-head">
      <button class="st-icone" onclick={botaoEsquerdo}
        aria-label={tela === 'root' ? 'Fechar' : 'Voltar'}>{tela === 'root' ? '✕' : '‹'}</button>
      <!-- tabindex=-1: alvo do foco na troca de tela, sem entrar na ordem do Tab. -->
      <h2 class="st-titulo" bind:this={tituloEl} tabindex="-1">{TITULO[tela]}</h2>
      <span class="st-icone st-vazio" aria-hidden="true"></span>
      {#if TELAS_DE_SERVIDOR.includes(telaAtual) && nomeAlvo}
        <!-- Sem isto nao da pra saber em que maquina se esta mexendo: o app roda no front de um
             servidor e a lista e agregada, entao a config aberta pode ser de outra maquina. -->
        <p class="st-sub">em {nomeAlvo}</p>
      {/if}
    </header>
    {@render corpo()}
  {/if}
</BottomSheet>
{/if}

{#snippet corpo()}
  {#if telaAtual === 'root'}
    {#each SECOES as secao (secao)}
      <p class="st-secao">{secao === 'Servidor' && nomeAlvo ? `Servidor · ${nomeAlvo}` : secao}</p>
      <div class="st-cartao">
        {#each LINHAS.filter((l) => l.secao === secao) as l (l.id)}
          <SettingsRow icone={l.icone} rotulo={l.rotulo} descricao={l.descricao}
            desabilitada={l.servidor && semServidor}
            motivo="escolha um servidor na lista pra configurar"
            onPick={() => onIrPara(l.id)} />
        {/each}
      </div>
    {/each}
  {:else if telaAtual === 'aparencia'}
    <AppearanceSettings podeAoVivo={isDesktop} onVerAoVivo={() => (aoVivo = true)} />
  {:else if telaAtual === 'motores'}
    <EnginesSettings targetServer={alvo} />
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
    color: var(--text-muted); font-size: var(--text-xs);
    text-transform: uppercase; letter-spacing: 0.05em;
  }
  /* Cartao arredondado com as linhas dentro, no formato do iOS. `--surface-card` entra no veu do
     papel de parede junto com o resto (CLAUDE.md, "Transparencia"). */
  .st-cartao {
    background: var(--surface-card);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .st-cartao > :global(button + button) { border-top: 1px solid var(--border-subtle); }

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
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
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
  .st-nav-icone { flex-shrink: 0; width: 1.4em; text-align: center; }
</style>

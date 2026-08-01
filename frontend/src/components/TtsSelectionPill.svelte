<script lang="ts">
  import { onMount } from 'svelte';
  import { ttsSelection, iniciarCapturaDeSelecao } from '../lib/ttsSelection.svelte';
  import { ouvirTexto } from '../lib/ouvir';
  import { ttsNarracao } from '../lib/ttsNarracao.svelte';
  import { PRESET_LER, PRESET_CODIGO, PRESET_FALA, presetPadrao } from '../lib/ttsPresets';

  let isDesktop = $state(false);

  onMount(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    isDesktop = mq.matches;
    const aoTrocar = (e: MediaQueryListEvent) => { isDesktop = e.matches; };
    mq.addEventListener('change', aoTrocar);
    const parar = iniciarCapturaDeSelecao();
    return () => { mq.removeEventListener('change', aoTrocar); parar(); };
  });

  const confirmar = (msg: string) => Promise.resolve(window.confirm(msg));

  // `engajado` (nao ttsSelection.ativa direto) e quem decide se o painel aparece. Motivo: fase 2
  // acrescenta um CAMPO DE TEXTO no painel, e tocar nele pra digitar rouba o foco da pagina — o que
  // colapsa a Selection API e dispara selectionchange (ttsSelection.ativa vira false NO MEIO da
  // digitacao). Com o painel amarrado a ttsSelection.ativa ele sumiria assim que o usuario tocasse
  // no proprio campo. `textoSel`/`blocosSel` sao uma FOTO da selecao tirada na transicao
  // false->true, pelo mesmo motivo: le-los de ttsSelection depois que o campo ganhou foco leria ''.
  let engajado = $state(false);
  let textoSel = $state('');
  let blocosSel = $state<string[]>([]);
  let instrucao = $state('');
  // Override explicito do preset "Ler como está". Necessario porque o VALOR desse preset e a
  // string vazia (mesma coisa que "campo intocado") — sem esta flag, tocar "Ler como está" com uma
  // selecao que TEM codigo virava um no-op: o campo voltava a '' e o padrao esperto (explicar
  // codigo, ver `efetiva` abaixo) reassumia na hora, como se o toque nunca tivesse acontecido.
  let preferirLerLiteral = $state(false);
  let prevAtiva = false;

  $effect(() => {
    const ativaAgora = ttsSelection.ativa;
    if (ativaAgora) {
      engajado = true;
      ttsSelection.setEngajado(true);
      // A foto SEGUE a selecao enquanto ela existe, e so congela quando `ativa` cai — que e o
      // momento em que o campo de instrucao rouba o foco e colapsa a Selection API.
      //
      // Antes isto so rodava na borda de subida, e o resultado era um bug feio no celular: arrastar
      // pra selecionar dispara `selectionchange` desde o PRIMEIRO caractere, entao a foto guardava
      // "6 car." e nunca mais crescia — o painel oferecia ler um pedaco minusculo do que a pessoa
      // tinha marcado. Na fase 1 o rotulo lia a selecao ao vivo e acertava; a foto da fase 2
      // congelava cedo demais.
      textoSel = ttsSelection.texto;
      blocosSel = ttsSelection.blocos;
      // Ja o estado do PAINEL (instrucao, preset, revisao pendente) so reinicia numa selecao nova,
      // nunca no meio de um arraste — senao o que a pessoa digitou sumiria a cada movimento.
      if (!prevAtiva) {
        instrucao = ttsNarracao.ultimaInstrucao;
        preferirLerLiteral = false;
        ttsNarracao.limpar();
      }
    }
    prevAtiva = ativaAgora;
  });

  const temCodigoSel = $derived(blocosSel.length > 0);
  const rotulo = $derived(`🔊 Ouvir · ${textoSel.length.toLocaleString('pt-BR')} car.`);
  // Instrucao que VAI valer se o usuario tocar "Ouvir" agora: texto livre sempre vence; senao, a
  // escolha explicita de "ler como esta"; senao o padrao esperto (explicar codigo quando ha bloco
  // de codigo, ler como esta senao).
  const efetiva = $derived(
    instrucao.trim() || (preferirLerLiteral ? PRESET_LER : presetPadrao(temCodigoSel)),
  );
  // Custo em caracteres que IRIAM pra Groq se a instrucao efetiva nao for "ler como esta" — mostrado
  // ANTES do toque, junto do rotulo: a Groq e barata, mas nao e gratis.
  const charsGroq = $derived(
    efetiva ? textoSel.length + blocosSel.reduce((n, b) => n + b.length, 0) + efetiva.length : 0,
  );

  // Arrastar o painel (so no desktop). Ele nasce colado no fim da selecao e as vezes tapa
  // justamente o que a pessoa quer ler enquanto decide a instrucao. Posicao vale ate fechar —
  // reabrir volta a ancorar na selecao nova, que e onde ela esta olhando.
  //
  // left/top, nunca transform: elemento fixo com transform pinta retangulo preto no WebKit durante
  // a rolagem por inercia (mesma regra do TtsBar). Alca propria em vez de arrastar pelo titulo,
  // que e um botao — senao todo arraste vira clique em "Ouvir".
  let arrX = $state<number | null>(null);
  let arrY = $state<number | null>(null);

  // Topo do painel flutuante, PRESO dentro da janela. Ele e ancorado pelo topo na posicao da
  // selecao; com a selecao perto do rodape e o painel crescendo (o texto adaptado pela Groq e bem
  // maior que os presets), ele descia pra fora da tela levando os botoes junto — sem alca, sem X,
  // sem saida. Recalcula sozinho quando a altura medida muda, que e exatamente quando ele cresce.
  const topoFlutuante = $derived.by(() => {
    const alturaJanela = typeof window === 'undefined' ? 0 : window.innerHeight;
    const teto = Math.max(8, alturaJanela - ttsSelection.panelH - 8);
    return Math.min(ttsSelection.y + 6, teto);
  });

  function iniciarArraste(e: PointerEvent) {
    if (!panelEl) return;
    e.preventDefault();
    const r = panelEl.getBoundingClientRect();
    const offX = e.clientX - r.left;
    const offY = e.clientY - r.top;
    const mover = (ev: PointerEvent) => {
      arrX = Math.max(8, Math.min(window.innerWidth - r.width - 8, ev.clientX - offX));
      arrY = Math.max(8, Math.min(window.innerHeight - r.height - 8, ev.clientY - offY));
    };
    const soltar = () => {
      window.removeEventListener('pointermove', mover);
      window.removeEventListener('pointerup', soltar);
    };
    window.addEventListener('pointermove', mover);
    window.addEventListener('pointerup', soltar);
  }

  function fechar() {
    ttsNarracao.limpar();
    ttsSelection.limpar();
    engajado = false;
    ttsSelection.setEngajado(false);
    arrX = null;
    arrY = null;
  }

  /** Toque principal: sem instrucao (ou so "ler como esta"), toca DIRETO — mesmo caminho sincrono
   * da fase 1, unlock do iOS incluido. Com instrucao, pede a revisao da Groq e SO ENTAO mostra o
   * botao que de fato toca (segundo toque, que e onde o unlock deste caminho acontece). */
  function ouvirClique() {
    if (!efetiva) {
      const texto = textoSel;
      ttsSelection.limpar();
      engajado = false;
      ttsSelection.setEngajado(false);
      ouvirTexto(texto, confirmar, '');
      return;
    }
    void ttsNarracao.pedir(textoSel, blocosSel, efetiva);
  }

  function confirmarLeitura() {
    const texto = ttsNarracao.textoTratado;
    const instrucaoUsada = ttsNarracao.instrucaoUsada;
    ttsNarracao.limpar();
    ttsSelection.limpar();
    engajado = false;
    ttsSelection.setEngajado(false);
    // Handler SINCRONO ate aqui: e o segundo toque que destrava o audio no iOS.
    ouvirTexto(texto, confirmar, instrucaoUsada);
  }

  // Mede a propria altura (ttsSelection.setPanelH -> App.svelte compoe --cp-tts-h dai) — mesmo
  // padrao do ttsPlayer.barH/TtsBar: a fase 1 cravava 52px, e presets + campo + revisao passam
  // longe disso.
  let panelEl = $state<HTMLDivElement | null>(null);
  $effect(() => {
    if (!panelEl) { ttsSelection.setPanelH(0); return; }
    const el = panelEl;
    let raf = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        ttsSelection.setPanelH(Math.round(el.getBoundingClientRect().height));
      });
    });
    ro.observe(el);
    return () => { cancelAnimationFrame(raf); ro.disconnect(); ttsSelection.setPanelH(0); };
  });
</script>

{#if engajado}
  <!-- Flutua so quando o alvo veio de uma selecao (tem x/y de verdade). O 🔊 da bolha abre a
       mensagem inteira sem ponto de toque nenhum pra ancorar — nos dois tamanhos de tela ele cai na
       mesma barra rente ao composer que o celular ja usa pra selecao (bottom calc abaixo). -->
  {@const flutua = isDesktop && ttsSelection.origem === 'selecao'}
  {@const movido = isDesktop && arrX !== null && arrY !== null}
  <div
    class="tts-sel"
    class:flutuante={flutua}
    class:movido
    style:right={movido ? undefined : (flutua ? `calc(100% - ${ttsSelection.x}px)` : undefined)}
    style:top={movido ? `${arrY}px` : (flutua ? `${topoFlutuante}px` : undefined)}
    style:left={movido ? `${arrX}px` : undefined}
    bind:this={panelEl}
  >
    <!-- Cabecalho SEMPRE presente, fora dos ramos de estado. Ele so existia no estado inicial, e o
         resultado foi o usuario preso: o texto revisado pela Groq cresceu, empurrou os botoes pra
         fora da tela, e nao havia nem alca pra arrastar nem X pra fechar naquele estado. -->
    <div class="tts-sel-top">
      {#if isDesktop}
        <span class="tts-sel-alca" onpointerdown={iniciarArraste} role="presentation" title="Arrastar">⠿</span>
      {/if}
      {#if ttsNarracao.carregando || ttsNarracao.erro || ttsNarracao.pendente}
        <span class="tts-sel-titulo">{ttsNarracao.pendente ? 'Texto adaptado' : 'Leitura em voz'}</span>
      {:else}
        <button type="button" class="tts-sel-head" onclick={ouvirClique}>{rotulo}</button>
      {/if}
      <button type="button" class="tts-sel-x" onclick={fechar} aria-label="Fechar">✕</button>
    </div>

    {#if ttsNarracao.carregando}
      <!-- Cancelar existe aqui porque a espera pela Groq pode chegar aos 60s do timeout do backend
           (narrar.py). Sem ele, uma consulta travada prende a faixa inteira: nao da pra tocar nada
           nem descartar o pedido. Nao aborta a requisicao em voo — so libera a interface, que e o
           que a pessoa precisa. -->
      <span class="tts-sel-load">consultando a Groq…</span>
    {:else if ttsNarracao.erro}
      <span class="tts-sel-err">{ttsNarracao.erro}</span>
    {:else if ttsNarracao.pendente}
      <p class="tts-sel-preview">{ttsNarracao.textoTratado}</p>
      <div class="tts-sel-row">
        <button type="button" class="tts-sel-btn tts-sel-go" onclick={confirmarLeitura}>🔊 Ouvir</button>
        <button type="button" class="tts-sel-btn" onclick={fechar}>Cancelar</button>
      </div>
    {:else}
      <div class="tts-sel-row">
        <!-- Adaptar pra fala e o PADRAO — nasce marcado, e o botao "Ouvir" de cima ja usa ele sem
             ninguem digitar nada. Aparece como atalho mesmo assim pra dar de volta a escolha depois
             de tocar "Ler como está". -->
        <button type="button" class="tts-sel-btn" class:sel={efetiva === PRESET_FALA}
                onclick={() => {
                  preferirLerLiteral = false;
                  instrucao = PRESET_FALA;
                  void ttsNarracao.pedir(textoSel, blocosSel, PRESET_FALA);
                }}>Adaptar pra fala</button>
        <!-- Os presets AGEM no clique, nao so marcam modo. "Ler como está" quer dizer "lê agora,
             sem LLM" — nao sobra nada pra configurar depois, entao exigir um segundo toque no botao
             de cima fazia o clique parecer morto. Sincrono ate ouvirTexto: e aqui que o audio
             destrava no iOS. -->
        <button type="button" class="tts-sel-btn" class:sel={efetiva === PRESET_LER}
                onclick={() => {
                  preferirLerLiteral = true;
                  instrucao = '';
                  const texto = textoSel;
                  ttsSelection.limpar();
                  engajado = false;
                  ttsSelection.setEngajado(false);
                  ouvirTexto(texto, confirmar, '');
                }}>Ler como está</button>
        {#if temCodigoSel}
          <!-- Idem: escolher "explicar o código" JA pede a explicacao. O segundo toque continua
               existindo depois, no botao Ouvir da tela de revisao, que e onde o unlock acontece
               neste caminho. -->
          <button type="button" class="tts-sel-btn" class:sel={efetiva === PRESET_CODIGO}
                  onclick={() => {
                    preferirLerLiteral = false;
                    instrucao = PRESET_CODIGO;
                    void ttsNarracao.pedir(textoSel, blocosSel, PRESET_CODIGO);
                  }}>Explicar o código</button>
        {/if}
      </div>
      <input
        class="tts-sel-input"
        type="text"
        bind:value={instrucao}
        oninput={() => { preferirLerLiteral = false; }}
        placeholder="ou digite uma instrução…"
        onkeydown={(e) => { if (e.key === 'Enter') ouvirClique(); }}
      />
      {#if efetiva}
        <span class="tts-sel-custo">{charsGroq.toLocaleString('pt-BR')} car. para a Groq</span>
      {/if}
    {/if}
  </div>
{/if}

<style>
  /* Celular: barra rente ao composer — nao disputa espaco com o menu nativo do Safari, que nasce
     colado na selecao. Desktop com selecao: painel flutuante no fim dela. Desktop pelo 🔊 da bolha
     (origem 'bolha', sem x/y): mesma barra do celular, ver `flutua` no template acima.
     Sem backdrop-filter/transform em barra fixa: no WebKit pinta retangulo preto na rolagem por
     inercia (mesmo motivo do TtsBar). Sem onpointerdown preventDefault: a selecao capturada ja foi
     fotografada em textoSel/blocosSel na abertura do painel, ninguem aqui le mais o DOM ao vivo. */
  .tts-sel {
    position: fixed;
    z-index: 39;
    display: flex;
    flex-direction: column;
    /* Escala do app (space, text, radius), nao pixel cru: o painel nasceu com 8px de padding, 13px
       de fonte e 14px de raio, todos chutados, e destoava de tudo em volta. */
    gap: var(--space-2);
    padding: var(--space-3);
    border-radius: var(--radius-lg);
    border: 1px solid var(--border-subtle);
    background: var(--surface-raised);
    color: var(--text-primary);
    font-size: var(--text-sm);
    width: min(calc(100vw - var(--space-8)), 340px);
    /* Teto absoluto: mesmo preso pelo topo, o painel nunca passa da janela. O texto adaptado rola
       dentro dele (.tts-sel-preview), os botoes ficam sempre alcancaveis. */
    max-height: calc(100vh - var(--space-4));
    /* --cp-tts-bar-h (publicada no App.svelte): soma a altura da BARRA DO PLAYER quando ela esta
       ativa, senao o painel nasce no mesmo lugar da TtsBar e tapa play/posicao/velocidade. */
    bottom: calc(var(--cp-dock-h, 150px) + 10px + var(--cp-tts-bar-h, 0px));
  }
  .tts-sel:not(.flutuante) {
    left: 0;
    right: 0;
    margin: 0 auto;
  }
  .tts-sel.flutuante {
    bottom: auto;
    margin-left: 0;
  }
  /* Arrastado: manda em left/top e larga as ancoras de baixo e da direita. */
  .tts-sel.movido { bottom: auto; right: auto; margin: 0; }
  .tts-sel-top { display: flex; align-items: center; gap: var(--space-2); }
  .tts-sel-alca {
    cursor: grab;
    color: var(--text-muted);
    font-size: var(--text-sm);
    line-height: 1;
    touch-action: none;
    user-select: none;
  }
  .tts-sel-alca:active { cursor: grabbing; }
  /* Botao PREENCHIDO, e nao texto com aparencia de titulo: e ele que de fato toca. Ficou assim
     depois de o usuario clicar em "Ler como está" (que e so o seletor de modo) esperando ouvir —
     o preset selecionado tinha preenchimento e virava a coisa mais parecida com botao na caixa. */
  .tts-sel-head {
    all: unset;
    cursor: pointer;
    flex: 1;
    min-width: 0;
    text-align: center;
    background: var(--accent);
    color: var(--text-inverse);
    border-radius: var(--radius-full);
    padding: var(--space-2) var(--space-3);
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .tts-sel-head:active { background: var(--accent-press); }
  .tts-sel-titulo {
    flex: 1;
    min-width: 0;
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-secondary);
  }
  .tts-sel-x {
    all: unset;
    cursor: pointer;
    padding: 0 var(--space-1);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }
  .tts-sel-x:hover { color: var(--text-primary); }
  .tts-sel-row { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .tts-sel-btn {
    border: 1px solid var(--border-subtle);
    background: transparent;
    color: var(--text-primary);
    border-radius: var(--radius-full);
    padding: var(--space-1) var(--space-3);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  /* Preset selecionado marca com contorno e cor de texto, SEM preenchimento: preenchimento e do
     botao que toca (.tts-sel-head). Dois preenchidos na mesma caixa e o que fez o usuario clicar no
     preset achando que ia ouvir. */
  .tts-sel-btn.sel { border-color: var(--accent); color: var(--accent); }
  .tts-sel-go { border-color: var(--accent); color: var(--accent); background: var(--accent-dim); font-weight: 600; }
  .tts-sel-input {
    background: var(--surface-inset);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    padding: var(--space-2);
    font-size: var(--text-sm);
  }
  .tts-sel-custo { color: var(--text-secondary); font-size: 11px; }
  .tts-sel-preview {
    margin: 0;
    max-height: 30vh;
    overflow-y: auto;
    white-space: pre-wrap;
    font-size: 13px;
  }
  .tts-sel-load, .tts-sel-err { color: var(--text-secondary); font-size: 13px; }
  .tts-sel-err { color: var(--error); }
</style>

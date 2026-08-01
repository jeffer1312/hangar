<script lang="ts">
  import { onMount } from 'svelte';
  import { ttsSelection, iniciarCapturaDeSelecao } from '../lib/ttsSelection.svelte';
  import { ouvirTexto } from '../lib/ouvir';
  import { ttsNarracao } from '../lib/ttsNarracao.svelte';
  import { PRESET_LER, PRESET_CODIGO, PRESET_FALA, presetPadrao } from '../lib/ttsPresets';

  // Sem matchMedia aqui: o painel e igual nas duas telas desde que passou a ficar colado no
  // composer. O que sobrou do onMount e so ligar a captura de selecao.
  onMount(() => iniciarCapturaDeSelecao());

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
  // Selecionar texto — pra copiar, pra reler, pra qualquer coisa — nao pode trazer preset, campo de
  // instrucao e custo na cara. Nasce so a pilha "Ouvir . N car."; o resto abre no ⌄, sob demanda.
  let expandido = $state(false);
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
        expandido = false;
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

  function fechar() {
    ttsNarracao.limpar();
    ttsSelection.limpar();
    engajado = false;
    ttsSelection.setEngajado(false);
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
  <!-- SEMPRE colado no composer, nas duas telas. Antes ele flutuava no fim da selecao no desktop, e
       o usuario reprovou: aparecia no meio da leitura e nao combinava com nada. Colado, ele ocupa o
       mesmo lugar que as pills que o Chat ja usa (hist-pill/tui-pill) e some do caminho do texto.
       Sem arrastar tambem: era resposta pro problema de ele tapar a leitura, que colado nao existe. -->
  <div class="tts-sel" class:aberto={expandido || ttsNarracao.carregando || ttsNarracao.erro || ttsNarracao.pendente} bind:this={panelEl}>
    <!-- Cabecalho SEMPRE presente, fora dos ramos de estado. Ele so existia no estado inicial, e o
         resultado foi o usuario preso: o texto revisado pela Groq cresceu, empurrou os botoes pra
         fora da tela, e nao havia nem X pra fechar naquele estado. -->
    <div class="tts-sel-top">
      {#if ttsNarracao.carregando || ttsNarracao.erro || ttsNarracao.pendente}
        <span class="tts-sel-titulo">{ttsNarracao.pendente ? 'Texto adaptado' : 'Leitura em voz'}</span>
      {:else}
        <button type="button" class="tts-sel-head" onclick={ouvirClique}>{rotulo}</button>
        <button type="button" class="tts-sel-mais" onclick={() => (expandido = !expandido)}
                aria-expanded={expandido} aria-label={expandido ? 'Menos opções' : 'Mais opções'}
        >{expandido ? '⌃' : '⌄'}</button>
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
    {:else if expandido}
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
    width: fit-content;
    max-width: min(calc(100vw - var(--space-8)), 420px);
    /* Teto absoluto: o texto adaptado rola dentro (.tts-sel-preview), os botoes ficam alcancaveis. */
    max-height: calc(100vh - var(--space-4));
    /* --cp-tts-bar-h (publicada no App.svelte): soma a altura da BARRA DO PLAYER quando ela esta
       ativa, senao o painel nasce no mesmo lugar da TtsBar e tapa play/posicao/velocidade. */
    bottom: calc(var(--cp-dock-h, 150px) + 10px + var(--cp-tts-bar-h, 0px));
    left: 0;
    right: 0;
    margin: 0 auto;
  }
  /* Fechado, e UMA PILULA — a mesma forma que o Chat ja usa acima do composer (hist-pill/tui-pill):
     raio total, contorno de destaque, sombra, uma linha so. A caixa quadrada com botao preenchido
     dentro nao existe em lugar nenhum deste app, e foi o que o usuario reprovou. */
  .tts-sel:not(.aberto) {
    flex-direction: row;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-full);
    border-color: var(--accent);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
  }
  .tts-sel-top { display: flex; align-items: center; gap: var(--space-2); }
  /* Fechado a propria pilula E o botao — nao ha botao preenchido dentro de caixa, que era o que
     destoava. Aberto ele vira o titulo da caixa e continua clicavel. */
  .tts-sel-head {
    all: unset;
    cursor: pointer;
    flex: 1;
    min-width: 0;
    font-size: var(--text-sm);
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tts-sel-head:active { color: var(--accent); }
  .tts-sel-titulo {
    flex: 1;
    min-width: 0;
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-secondary);
  }
  .tts-sel-mais {
    all: unset;
    cursor: pointer;
    padding: 0 var(--space-1);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }
  .tts-sel-mais:hover { color: var(--text-primary); }
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

<script lang="ts">
  import ThemeToggle from '../ThemeToggle.svelte';
  import BackgroundToggle from '../BackgroundToggle.svelte';
  import SegmentedPicker from '../SegmentedPicker.svelte';
  import AparenciaAmostra from './AparenciaAmostra.svelte';
  import {
    getReadMode, setReadMode, getPanelStyle, setPanelStyle,
    getReadAlpha, setReadAlpha, getTextBoost, setTextBoost,
    getFontPref, setFontPref, getMedidaTexto, setMedidaTexto,
    getSurfaceSolid, setSurfaceSolid,
    READ_ALPHA_PADRAO, TEXT_BOOST_PADRAO, SURFACE_SOLID_PADRAO,
    type ReadMode, type PanelStyle, type FontPref, type MedidaTexto,
  } from '../../lib/background';
  import { sidebarPrefs, type SidebarHeight } from '../../lib/sidebarPrefs.svelte';

  interface Props {
    /** Desktop: oferece o botao "Ver ao vivo", que troca o painel pela caixinha flutuante. */
    podeAoVivo?: boolean;
    onVerAoVivo?: () => void;
    /** Ja estamos DENTRO da caixinha: a previa embutida sai (o chat de verdade esta a vista atras). */
    semPrevia?: boolean;
  }
  let { podeAoVivo = false, onVerAoVivo, semPrevia = false }: Props = $props();

  let leitura = $state<ReadMode>(getReadMode());
  let paineis = $state<PanelStyle>(getPanelStyle());
  let solidez = $state(getReadAlpha());
  let contraste = $state(getTextBoost());
  let fonte = $state<FontPref>(getFontPref());
  let texto = $state<Record<MedidaTexto, number>>({
    size: getMedidaTexto('size'), lh: getMedidaTexto('lh'), width: getMedidaTexto('width'),
  });
  // Breakpoint desktop, reativo e nao um retrato do boot: atravessar os 820px (girar o tablet,
  // redimensionar a janela) precisa aparecer/sumir com o slider de largura na hora.
  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });

  const medidasTexto: { v: MedidaTexto; label: string; aria: string }[] = [
    { v: 'size', label: 'Tamanho', aria: 'Tamanho do texto da conversa' },
    { v: 'lh', label: 'Entrelinha', aria: 'Espaço entre as linhas da conversa' },
    { v: 'width', label: 'Largura da coluna', aria: 'Largura da coluna de leitura' },
  ];

  const opcoesLeitura: { v: ReadMode; label: string; aria: string }[] = [
    { v: 'auto', label: 'Automática', aria: 'Reforça o texto só quando o fundo é uma imagem' },
    { v: 'glass', label: 'Nenhum', aria: 'Nada muda na conversa' },
    { v: 'text', label: 'Texto', aria: 'Sem caixa: só o texto ganha contraste e sombra' },
    { v: 'solid', label: 'Folha', aria: 'A conversa inteira numa folha opaca' },
  ];
  const opcoesFonte: { v: FontPref; label: string; aria: string }[] = [
    { v: 'system', label: 'Sistema', aria: 'A fonte de interface do sistema' },
    { v: 'mono', label: 'Monoespaçada', aria: 'A fonte de largura fixa, como no terminal' },
  ];
  const opcoesPaineis: { v: PanelStyle; label: string; aria: string }[] = [
    { v: 'card', label: 'Caixa solta', aria: 'Painéis flutuando, com folga e cantos redondos' },
    { v: 'edge', label: 'Colados', aria: 'Painéis colados na borda da tela, de ponta a ponta' },
  ];
  let resetSeq = $state(0);
  // Espelho da "Solidez das caixas" (o slider vive no BackgroundToggle) so pra o botao saber se ha o
  // que desfazer. Reler no reset basta: quem edita o valor e o proprio BackgroundToggle, que remonta
  // junto pelo `{#key resetSeq}`.
  let caixas = $state(getSurfaceSolid());
  $effect(() => { resetSeq; caixas = getSurfaceSolid(); });

  // VOLTAR AO PADRAO: so as medidas que a amostra mostra. NAO mexe em tema, fonte, papel de parede
  // nem paineis — quem arrasta sliders e se perde quer desfazer os sliders, nao perder a foto que
  // escolheu.
  //
  // Os valores vem das CONSTANTES exportadas por lib/background, nunca de numero escrito aqui: a
  // primeira versao chutou 50 pro contraste e 12 pra solidez da folha, quando os de fabrica sao 10 e
  // 92. O estrago era silencioso e dobrado — o botao nascia habilitado numa instalacao virgem (92 !=
  // 12) e, clicado, DERRUBAVA a solidez da folha de 92 pra 12, uma mudanca visual grande que ninguem
  // pediu.
  const temAjuste = $derived(
    texto.size !== 100 || texto.lh !== 100 || texto.width !== 100 ||
    contraste !== TEXT_BOOST_PADRAO || solidez !== READ_ALPHA_PADRAO || leitura !== 'auto' ||
    caixas !== SURFACE_SOLID_PADRAO,
  );
  function voltarAoPadrao() {
    (['size', 'lh', 'width'] as MedidaTexto[]).forEach((m) => { texto[m] = 100; setMedidaTexto(m, 100); });
    contraste = TEXT_BOOST_PADRAO; setTextBoost(TEXT_BOOST_PADRAO);
    solidez = READ_ALPHA_PADRAO; setReadAlpha(READ_ALPHA_PADRAO);
    leitura = 'auto'; setReadMode('auto');
    caixas = SURFACE_SOLID_PADRAO; setSurfaceSolid(SURFACE_SOLID_PADRAO);
    // O slider "Solidez das caixas" vive no BackgroundToggle, que guarda o PROPRIO $state. Sem
    // remontar, o valor aplicado voltava ao padrao mas o slider de la seguia mostrando o numero
    // antigo ate reabrir a tela — a tela mentindo sobre o proprio estado.
    resetSeq += 1;
  }

  const opcoesAltura: { v: SidebarHeight; label: string; aria: string }[] = [
    { v: 'full', label: 'Altura total', aria: 'A barra lateral vai de ponta a ponta da tela' },
    { v: 'content', label: 'Só o conteúdo', aria: 'A barra lateral encolhe até a altura das sessões' },
  ];
</script>

<div class="ap-sheet">
  <!-- Amostra GRUDADA no topo: todo slider daqui muda a conversa, que fica atras deste painel (e no
       celular nao ha "atras" nenhum — o painel e a tela). Sticky pra ela continuar a vista enquanto
       se rola ate os sliders la embaixo. -->
  <!-- Grudado no topo SO quando ha previa: na caixinha do "ao vivo" a previa nao existe (o chat de
       verdade esta a vista atras), e um botao sozinho preso no topo com fundo translucido virava uma
       tarja por cima do texto que rolava embaixo. -->
  <div class="ap-amostra" class:ap-amostra--solta={semPrevia}>
    {#if !semPrevia}<AparenciaAmostra />{/if}
    <div class="ap-acoes">
      {#if podeAoVivo}
        <!-- Desktop: a previa embutida e uma amostra; isto revela a conversa DE VERDADE atras,
             encolhendo o painel numa caixinha no canto. -->
        <button class="ap-padrao" onclick={onVerAoVivo}>Ver ao vivo</button>
      {/if}
      <button class="ap-padrao" onclick={voltarAoPadrao} disabled={!temAjuste}>
        Voltar ao padrão
      </button>
    </div>
  </div>

  <div class="ap-row">
    <div class="ap-label">
      <strong>Tema</strong>
      <span>claro, escuro ou o do sistema</span>
    </div>
    <ThemeToggle />
  </div>

  <div class="ap-row ap-row--stack">
    <div class="ap-label">
      <strong>Fundo</strong>
      <span>textura, luz ou uma imagem sua — guardada só neste dispositivo</span>
    </div>
    <!-- `{#key}`: o "Voltar ao padrao" grava a solidez das caixas, mas o slider vive aqui dentro com
         estado proprio — remontar e o que faz o numero na tela bater com o valor aplicado. -->
    {#key resetSeq}<BackgroundToggle />{/key}
  </div>

  <div class="ap-row ap-row--stack">
    <div class="ap-head">
      <div class="ap-label">
        <strong>Leitura</strong>
        <span>o que segura o texto quando há foto de fundo: reforçar só o texto ou pôr a conversa numa folha</span>
      </div>
      <SegmentedPicker value={leitura} options={opcoesLeitura} ariaLabel="Leitura"
                       onPick={(v) => { leitura = v; setReadMode(v); }} />
    </div>
    {#if leitura !== 'glass'}
      <!-- Mesma lógica do slider do fundo: 100 tapa a foto atrás da conversa, 0 deixa ela passar
           inteira. "Sólida" no talo virava um bloco escuro — o ponto certo é olhando. -->
      <label class="ap-slider">
        <span>{leitura === 'solid' ? 'Solidez da folha' : 'Força'}</span>
        <input type="range" min="0" max="100" step="1" value={solidez}
               oninput={(e) => { solidez = +(e.currentTarget as HTMLInputElement).value; setReadAlpha(solidez); }} />
        <em>{solidez}</em>
      </label>
    {/if}
    {#if leitura === 'text' || leitura === 'auto'}
      <!-- Contraste do texto: os tokens do app são propositalmente mais escuros que branco (conforto
           em sessão longa); sobre foto isso não vale, e aqui você escolhe quanto do branco volta. -->
      <label class="ap-slider">
        <span>Contraste do texto</span>
        <input type="range" min="0" max="100" step="1" value={contraste}
               oninput={(e) => { contraste = +(e.currentTarget as HTMLInputElement).value; setTextBoost(contraste); }} />
        <em>{contraste}</em>
      </label>
    {/if}
  </div>

  <!-- As tres medidas que decidem o conforto de leitura, cada uma como escala (100 = o de hoje).
       A LARGURA so aparece no desktop: no celular a coluna e a tela inteira e o slider nao teria o
       que mover. Medido nesta conversa: 43 caracteres por linha no celular contra 93 no desktop —
       a faixa confortavel de leitura e 45 a 75, e e por isso que o mesmo texto cansa mais no monitor. -->
  <div class="ap-row ap-row--stack">
    <div class="ap-label">
      <strong>Texto da conversa</strong>
      <span>tamanho, entrelinha e largura da coluna — 100 é como vem de fábrica</span>
    </div>
    {#each medidasTexto as m (m.v)}
      {#if m.v !== 'width' || isDesktop}
        <label class="ap-slider">
          <span>{m.label}</span>
          <input type="range" min="50" max="150" step="1" value={texto[m.v]}
                 aria-label={m.aria}
                 oninput={(e) => { const n = +(e.currentTarget as HTMLInputElement).value; texto[m.v] = n; setMedidaTexto(m.v, n); }} />
          <em>{texto[m.v]}</em>
        </label>
      {/if}
    {/each}
  </div>

  <!-- Fonte: nenhuma API web lê a fonte do terminal, então a escolha é aqui. 'Monoespaçada' usa
       `--font-mono`, que nomeia JetBrains e cai no ui-monospace de quem não tiver. O TAMANHO tem
       controle proprio logo acima (escala sobre o padrao da tela), que convive com o zoom do
       navegador em vez de brigar com ele. -->
  <div class="ap-row">
    <div class="ap-label">
      <strong>Fonte</strong>
      <span>a do sistema ou a de largura fixa, como no terminal</span>
    </div>
    <SegmentedPicker value={fonte} options={opcoesFonte} ariaLabel="Fonte"
                     onPick={(v) => { fonte = v; setFontPref(v); }} />
  </div>

  <div class="ap-row">
    <div class="ap-label">
      <strong>Painéis</strong>
      <span>contexto e aparência como caixa flutuante ou colados na borda</span>
    </div>
    <SegmentedPicker value={paineis} options={opcoesPaineis} ariaLabel="Painéis"
                     onPick={(v) => { paineis = v; setPanelStyle(v); }} />
  </div>

  <!-- Barra lateral: só existe no desktop (no celular a lista é a tela inteira), então a seção some
       abaixo de 820px em vez de oferecer um ajuste que não muda nada.
       Abrir e fechar a barra é o botão dela mesma — aqui fica só a altura. -->
  <div class="ap-row ap-row--desktop">
    <div class="ap-label">
      <strong>Altura da barra lateral</strong>
      <span>de ponta a ponta, ou encolhida até onde as sessões terminam</span>
    </div>
    <SegmentedPicker value={sidebarPrefs.height} options={opcoesAltura} ariaLabel="Altura da barra lateral"
                     onPick={(v) => (sidebarPrefs.height = v)} />
  </div>
</div>

<style>
  /* Container query, nao media query: quem aperta a linha e a largura do PAINEL, nao a da janela.
     No desktop o dock tem ~530px numa tela de 1440 — uma media query de 560px nunca dispararia ali,
     e era exatamente onde a descricao quebrava em palavras soltas ao lado do segmentado. O painel
     ainda e redimensionavel, entao o corte tem que seguir a largura de verdade. */
  .ap-sheet { container-type: inline-size; }

  /* Sticky com o fundo do painel por tras: sem o fundo, o texto dos sliders passaria POR CIMA da
     amostra ao rolar, e a amostra existe justamente pra ser um pedaco fiel da conversa. */
  .ap-amostra {
    position: sticky;
    top: 0;
    z-index: 2;
    padding-bottom: var(--space-2);
    background: var(--glass-panel, var(--bg-surface));
  }
  .ap-amostra--solta { position: static; background: none; padding-bottom: var(--space-3); }
  .ap-acoes { display: flex; gap: var(--space-2); }
  .ap-padrao {
    flex: 1;
    width: 100%;
    height: 34px;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--surface-raised);
    color: var(--text-secondary);
    font-size: var(--text-xs);
  }
  .ap-padrao:disabled { opacity: 0.45; cursor: default; }
  @media (hover: hover) { .ap-padrao:not(:disabled):hover { color: var(--text-primary); background: var(--bg-hover); } }
  .ap-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border-subtle);
  }
  /* O bloco do fundo tem seletor + slider + links: em coluna ele respira, em linha aperta tudo. */
  .ap-row--stack { flex-direction: column; align-items: stretch; }
  .ap-row:last-child { border-bottom: 0; }
  /* Ajuste que só tem efeito no desktop (a barra lateral não existe no celular). */
  @media (max-width: 819px) { .ap-row--desktop { display: none; } }
  .ap-label { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .ap-label strong { color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .ap-label span { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4; }
  /* Linha com título à esquerda e segmentado à direita, e o slider embaixo ocupando a largura. */
  .ap-head { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-3); }
  /* O segmentado nao encolhe e o rotulo tem `min-width: 0`, entao ele cede TUDO: abaixo desta
     largura a descricao virava uma palavra por linha. Empilha antes de chegar la. Vale pras duas
     views — no celular a folha e a tela, no desktop e o painel. */
  @container (max-width: 560px) {
    .ap-head, .ap-row { flex-direction: column; align-items: stretch; }
  }
  .ap-slider { display: flex; align-items: center; gap: var(--space-2); margin-top: var(--space-2); }
  .ap-slider span { color: var(--text-muted); font-size: var(--text-xs); white-space: nowrap; }
  .ap-slider input { flex: 1; min-width: 120px; accent-color: var(--accent); }
  .ap-slider em { color: var(--text-muted); font-size: var(--text-xs); font-style: normal; min-width: 2ch; text-align: right; }
</style>

<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import ThemeToggle from './ThemeToggle.svelte';
  import BackgroundToggle from './BackgroundToggle.svelte';
  import SegmentedPicker from './SegmentedPicker.svelte';
  import {
    getReadMode, setReadMode, getPanelStyle, setPanelStyle,
    getReadAlpha, setReadAlpha, getTextBoost, setTextBoost,
    getFontPref, setFontPref, getMedidaTexto, setMedidaTexto,
    type ReadMode, type PanelStyle, type FontPref, type MedidaTexto,
  } from '../lib/background';
  import { sidebarPrefs, type SidebarHeight } from '../lib/sidebarPrefs.svelte';

  interface Props {
    open: boolean;
    onClose: () => void;
  }
  let { open, onClose }: Props = $props();

  let leitura = $state<ReadMode>(getReadMode());
  let paineis = $state<PanelStyle>(getPanelStyle());
  let solidez = $state(getReadAlpha());
  let contraste = $state(getTextBoost());
  let fonte = $state<FontPref>(getFontPref());
  let texto = $state<Record<MedidaTexto, number>>({
    size: getMedidaTexto('size'), lh: getMedidaTexto('lh'), width: getMedidaTexto('width'),
  });
  // Breakpoint desktop, no mesmo idioma do EnginesSheet: reativo, nao um retrato do boot —
  // atravessar os 820px (girar o tablet, redimensionar a janela) precisa trocar o formato na hora.
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
  const opcoesAltura: { v: SidebarHeight; label: string; aria: string }[] = [
    { v: 'full', label: 'Altura total', aria: 'A barra lateral vai de ponta a ponta da tela' },
    { v: 'content', label: 'Só o conteúdo', aria: 'A barra lateral encolhe até a altura das sessões' },
  ];
</script>

<!-- Aparência ganhou sheet própria: tema + fundo + transparência + imagem já não cabiam no menu da
     conta sem empurrar servidores e notificações pra fora da tela. Mesma mecânica dos outros
     painéis (doca à direita no desktop, sobe de baixo no celular). -->
<!-- `resizable` + largura própria: os 420px padrão picavam "claro, escuro ou o do sistema" em quatro
     linhas e espremiam a fileira Liso/Textura/Luz/Imagem. Chave só desta sheet — o painel do par e o
     do git guardam a largura deles. -->
<!-- `persistent`: aparência se ajusta OLHANDO o app — igual às "Configurações rápidas" do Gmail, o
     painel fica de lado, sem véu, e um clique no chat não o mata. Sai no × ou no Esc. -->
<!-- No desktop vira MODAL centrado e largo, nao dock estreito: sao 8 secoes de rotulo+controle, e
     num painel de ~530px a descricao de cada uma disputava a linha com o segmentado e quebrava em
     palavras soltas. Mesmo par `wide`+`centered` do EnginesSheet e do modal de git. -->
<BottomSheet {open} {onClose} ariaLabel="Aparência" persistent resizable widthKey="cp_appearance_w" defaultWidth={560}
             wide={isDesktop} centered={isDesktop}>
  <div class="ap-sheet">
  <h2 class="sheet-title">Aparência</h2>

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
    <BackgroundToggle />
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
</BottomSheet>

<style>
  .sheet-title {
    margin: 0 0 var(--space-4);
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
  }
  /* Container query, nao media query: quem aperta a linha e a largura do PAINEL, nao a da janela.
     No desktop o dock tem ~530px numa tela de 1440 — uma media query de 560px nunca dispararia ali,
     e era exatamente onde a descricao quebrava em palavras soltas ao lado do segmentado. O painel
     ainda e redimensionavel, entao o corte tem que seguir a largura de verdade. */
  .ap-sheet { container-type: inline-size; }
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

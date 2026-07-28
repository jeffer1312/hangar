<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import ThemeToggle from './ThemeToggle.svelte';
  import BackgroundToggle from './BackgroundToggle.svelte';
  import SegmentedPicker from './SegmentedPicker.svelte';
  import {
    getReadMode, setReadMode, getPanelStyle, setPanelStyle,
    getReadAlpha, setReadAlpha, getTextBoost, setTextBoost,
    type ReadMode, type PanelStyle,
  } from '../lib/background';

  interface Props {
    open: boolean;
    onClose: () => void;
  }
  let { open, onClose }: Props = $props();

  let leitura = $state<ReadMode>(getReadMode());
  let paineis = $state<PanelStyle>(getPanelStyle());
  let solidez = $state(getReadAlpha());
  let contraste = $state(getTextBoost());

  const opcoesLeitura: { v: ReadMode; label: string; aria: string }[] = [
    { v: 'auto', label: 'Automática', aria: 'Reforça o texto só quando o fundo é uma imagem' },
    { v: 'glass', label: 'Nenhum', aria: 'Nada muda na conversa' },
    { v: 'text', label: 'Texto', aria: 'Sem caixa: só o texto ganha contraste e sombra' },
    { v: 'solid', label: 'Folha', aria: 'A conversa inteira numa folha opaca' },
  ];
  const opcoesPaineis: { v: PanelStyle; label: string; aria: string }[] = [
    { v: 'card', label: 'Caixa solta', aria: 'Painéis flutuando, com folga e cantos redondos' },
    { v: 'edge', label: 'Colados', aria: 'Painéis colados na borda da tela, de ponta a ponta' },
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
<BottomSheet {open} {onClose} ariaLabel="Aparência" persistent resizable widthKey="cp_appearance_w" defaultWidth={560}>
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

  <div class="ap-row">
    <div class="ap-label">
      <strong>Painéis</strong>
      <span>contexto e aparência como caixa flutuante ou colados na borda</span>
    </div>
    <SegmentedPicker value={paineis} options={opcoesPaineis} ariaLabel="Painéis"
                     onPick={(v) => { paineis = v; setPanelStyle(v); }} />
  </div>
</BottomSheet>

<style>
  .sheet-title {
    margin: 0 0 var(--space-4);
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
  }
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
  .ap-label { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .ap-label strong { color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .ap-label span { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4; }
  /* Linha com título à esquerda e segmentado à direita, e o slider embaixo ocupando a largura. */
  .ap-head { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-3); }
  .ap-slider { display: flex; align-items: center; gap: var(--space-2); margin-top: var(--space-2); }
  .ap-slider span { color: var(--text-muted); font-size: var(--text-xs); white-space: nowrap; }
  .ap-slider input { flex: 1; min-width: 120px; accent-color: var(--accent); }
  .ap-slider em { color: var(--text-muted); font-size: var(--text-xs); font-style: normal; min-width: 2ch; text-align: right; }
</style>

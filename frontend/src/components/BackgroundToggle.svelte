<script lang="ts">
  import { getBgPref, setBgPref, setBgImage, clearBgImage, getBgImage, getBgScrim, setBgScrim, getSurfaceSolid, setSurfaceSolid, isShell, type BgPref } from '../lib/background';

  interface Props {
    /** A escolha vive aqui dentro ($state proprio); quem precisa saber QUAL pref esta ativa agora
        (AppearanceSettings, pra sumir com o desfoque no modo desktop) recebe pelo callback. */
    onEscolha?: (p: BgPref) => void;
  }
  let { onEscolha }: Props = $props();

  let pref = $state<BgPref>(getBgPref());
  let temImagem = $state(!!getBgImage());
  let erro = $state('');
  let arquivoEl = $state<HTMLInputElement | null>(null);
  let scrim = $state(getBgScrim());
  let solidez = $state(getSurfaceSolid());

  function pick(p: BgPref) {
    // 'image' sem imagem escolhida abre o seletor em vez de virar um modo que nao pinta nada.
    if (p === 'image' && !temImagem) { arquivoEl?.click(); return; }
    pref = p;
    setBgPref(p);
    onEscolha?.(p);
  }

  async function escolher(e: Event) {
    const f = (e.currentTarget as HTMLInputElement).files?.[0];
    if (!f) return;
    erro = '';
    try {
      await setBgImage(f);      // encolhe (lib/imagePrep) e guarda no proprio dispositivo
      temImagem = true;
      pref = 'image';
    } catch (e) {
      // Tres causas distintas caem aqui e a mensagem precisa dizer QUAL: cota do localStorage,
      // arquivo que o navegador nao decodifica (HEIC sem suporte, PNG corrompido) e canvas
      // indisponivel. Culpar sempre a cota mandava o usuario diminuir uma imagem que o problema
      // nunca foi o tamanho. O erro cru vai pro console: sem isso nao ha o que investigar depois.
      console.error('[fundo] falha ao aplicar a imagem:', e);
      const cota = (e instanceof DOMException && /quota/i.test(e.name))
        || (e instanceof Error && /grande demais/.test(e.message));
      erro = cota
        ? 'não coube no armazenamento do navegador; tente uma imagem menor'
        : 'não consegui ler essa imagem (formato não suportado neste navegador?)';
    }
  }

  function remover() {
    clearBgImage();
    temImagem = false;
    pref = getBgPref();
  }

  // Mesmo formato do ThemeToggle: segmentado curto, escolha imediata, sem confirmar.
  // "Desktop" só existe dentro do shell Electron: no navegador não há área de trabalho atrás da
  // janela, e oferecer a opção deixaria a tela sem fundo nenhum. getBgPref() também derruba
  // 'desktop' pra 'flat' se a preferência sobreviver num perfil que não é do shell.
  const opts: { v: BgPref; label: string; aria: string }[] = [
    { v: 'flat', label: 'Liso', aria: 'Fundo chapado' },
    { v: 'texture', label: 'Textura', aria: 'Fundo com grão e gradiente' },
    { v: 'aurora', label: 'Luz', aria: 'Fundo com grão, gradiente e uma luz no canto' },
    { v: 'image', label: 'Imagem', aria: 'Usar uma imagem de fundo' },
    ...(isShell()
      ? [{ v: 'desktop' as BgPref, label: 'Desktop', aria: 'Deixar a área de trabalho aparecer atrás' }]
      : []),
  ];
</script>

<div class="bg-wrap">
  <div class="bg-toggle" role="group" aria-label="Fundo">
    {#each opts as o (o.v)}
      <button
        class="bg-opt"
        class:active={pref === o.v}
        onclick={() => pick(o.v)}
        aria-pressed={pref === o.v}
        aria-label={o.aria}
        title={o.aria}
      >{o.label}</button>
    {/each}
  </div>

  <!-- O navegador nao enxerga o wallpaper do sistema (o terminal so consegue por ser translucido),
       entao a imagem e escolhida aqui e fica guardada neste dispositivo. -->
  <input bind:this={arquivoEl} type="file" accept="image/*" class="bg-file" onchange={escolher} aria-label="Escolher imagem de fundo" />
  {#if (pref === 'image' && temImagem) || pref === 'desktop'}
    <!-- Transparência: o equivalente ao que o compositor faz no terminal. Aplica ao arrastar (sem
         confirmar), porque a escolha só se faz olhando. -->
    <label class="bg-scrim">
      <span>Transparência</span>
      <input type="range" min="0" max="100" step="1"
             value={scrim}
             oninput={(e) => { scrim = +(e.currentTarget as HTMLInputElement).value; setBgScrim(scrim); }}
             aria-label="Transparência do fundo" />
      <em>{scrim}</em>
    </label>
    <!-- Solidez: a Transparência acima governa o painel; esta governa as CAIXAS de dentro dele
         (chip, campo, card, bloco de saída). Duas camadas, dois controles — no 0 as caixas somem no
         vidro e a tela vira uma superfície só; no 100 voltam a ser recorte chapado sobre a foto. -->
    <label class="bg-scrim">
      <span>Solidez das caixas</span>
      <input type="range" min="0" max="100" step="1"
             value={solidez}
             oninput={(e) => { solidez = +(e.currentTarget as HTMLInputElement).value; setSurfaceSolid(solidez); }}
             aria-label="Solidez das caixas sobre o fundo" />
      <em>{solidez}</em>
    </label>
  {/if}
  {#if pref === 'image' || temImagem}
    <div class="bg-img-row">
      <button class="bg-link" onclick={() => arquivoEl?.click()}>trocar imagem</button>
      {#if temImagem}<button class="bg-link danger" onclick={remover}>remover</button>{/if}
    </div>
  {/if}
  {#if erro}<p class="bg-erro">⚠ {erro}</p>{/if}
</div>

<style>
  .bg-wrap { display: flex; flex-direction: column; align-items: flex-end; gap: var(--space-1); }
  .bg-file { display: none; }
  .bg-scrim { display: flex; align-items: center; gap: var(--space-2); width: 100%; }
  .bg-scrim span { color: var(--text-muted); font-size: var(--text-xs); white-space: nowrap; }
  .bg-scrim input { flex: 1; min-width: 120px; accent-color: var(--accent); }
  /* Mesmo valor à direita dos sliders de Leitura (AppearanceSettings): sem o número não há como saber
     em quanto o fundo está, nem repetir um ponto que ficou bom. */
  .bg-scrim em { color: var(--text-muted); font-size: var(--text-xs); font-style: normal; min-width: 3ch; text-align: right; }
  .bg-img-row { display: flex; gap: var(--space-2); }
  .bg-link {
    min-height: 0; padding: 0; color: var(--text-muted); font-size: var(--text-xs);
    text-decoration: underline; text-underline-offset: 3px; text-decoration-color: var(--border-default);
  }
  .bg-link:hover { color: var(--text-secondary); }
  .bg-link.danger:hover { color: var(--error); }
  .bg-erro { margin: 0; color: var(--warning); font-size: var(--text-xs); max-width: 220px; text-align: right; }

  .bg-toggle {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .bg-opt {
    min-height: 32px;
    min-width: 0;
    padding: 0 var(--space-3);
    border-radius: 9px;
    font-size: var(--text-sm);
    color: var(--text-secondary);
    white-space: nowrap;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }
  .bg-opt:hover { color: var(--text-primary); }
  .bg-opt.active {
    background: var(--bg-elevated);
    color: var(--text-primary);
    box-shadow: inset 0 0 0 1px var(--border-default);
  }
</style>

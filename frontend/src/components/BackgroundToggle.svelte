<script lang="ts">
  import { getBgPref, setBgPref, setBgImage, clearBgImage, getBgImage, getBgScrim, setBgScrim, getSurfaceSolid, setSurfaceSolid, isShell, type BgPref } from '../lib/background';
  import * as m from '../paraglide/messages';

  interface Props {
    /** A escolha vive aqui dentro ($state proprio); quem precisa saber QUAL pref esta ativa agora
        (AppearanceSettings, pra sumir com o desfoque no modo desktop) recebe pelo callback. */
    onEscolha?: (p: BgPref) => void;
  }
  let { onEscolha }: Props = $props();

  // Id por instância: este componente é montado na tela de Aparência e em cada folha de troca de
  // sessão ao mesmo tempo, e id repetido faz o `aria-describedby` de uma instância resolver no
  // parágrafo da outra. Mesmo recurso que o `Select.svelte` da casa já usa.
  const uid = $props.id();

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
      console.error(m.config_fundo_falha_aplicar(), e);
      const cota = (e instanceof DOMException && /quota/i.test(e.name))
        || (e instanceof Error && /grande demais/.test(e.message));
      erro = cota
        ? m.config_fundo_erro_cota()
        : m.config_fundo_erro_leitura();
    }
  }

  function remover() {
    clearBgImage();
    temImagem = false;
    pref = getBgPref();
  }

  // Mesmo formato do ThemeToggle: segmentado curto, escolha imediata, sem confirmar.
  // "Desktop" só funciona dentro do shell Electron: no navegador não há área de trabalho atrás da
  // janela. Fora dele a opção continua na fileira, desabilitada e com o motivo escrito — some
  // ensinava que ela não existe. getBgPref() também derruba 'desktop' pra 'flat' se a preferência
  // sobreviver num perfil que não é do shell.
  const dentroDoShell = $derived(isShell());
  const opts = $derived<{ v: BgPref; label: string; aria: string; off: boolean }[]>([
    { v: 'flat', label: m.config_fundo_liso(), aria: m.config_fundo_chapado(), off: false },
    { v: 'texture', label: m.config_fundo_textura(), aria: m.config_fundo_grao(), off: false },
    { v: 'aurora', label: m.config_fundo_luz(), aria: m.config_fundo_aurora(), off: false },
    { v: 'image', label: m.config_fundo_imagem(), aria: m.config_fundo_usar_imagem(), off: false },
    { v: 'desktop', label: m.config_aparencia_desktop(), aria: m.config_fundo_desktop(), off: !dentroDoShell },
  ]);

  // Transparência e Solidez governam a foto de fundo: sem foto (ou sem desktop) não há o que
  // ajustar, mas os sliders ficam à vista, desabilitados, com o motivo.
  const temFundoDeFoto = $derived((pref === 'image' && temImagem) || pref === 'desktop');
</script>

<div class="bg-wrap">
  <div class="bg-toggle" role="group" aria-label={m.config_fundo_curto()}>
    {#each opts as o (o.v)}
      <button
        class="bg-opt"
        class:active={pref === o.v}
        onclick={() => pick(o.v)}
        disabled={o.off}
        aria-pressed={pref === o.v}
        aria-label={o.aria}
        aria-describedby={o.off ? `${uid}-desktop` : undefined}
        title={o.aria}
      >{o.label}</button>
    {/each}
  </div>
  {#if !dentroDoShell}
    <!-- Mesma razão do ThemeToggle: sem o vínculo, o botão apagado se anuncia sem o porquê. -->
    <p class="bg-motivo" id="{uid}-desktop">{m.config_aparencia_motivo_fundo_desktop()}</p>
  {/if}

  <!-- O navegador nao enxerga o wallpaper do sistema (o terminal so consegue por ser translucido),
       entao a imagem e escolhida aqui e fica guardada neste dispositivo. -->
  <input bind:this={arquivoEl} type="file" accept="image/*" class="bg-file" onchange={escolher} aria-label={m.config_fundo_escolher()} />
  <!-- Transparência: o equivalente ao que o compositor faz no terminal. Aplica ao arrastar (sem
       confirmar), porque a escolha só se faz olhando. -->
  <label class="bg-scrim" class:off={!temFundoDeFoto}>
    <span>{m.config_fundo_transparencia()}</span>
    <input type="range" min="0" max="100" step="1"
           value={scrim}
           disabled={!temFundoDeFoto}
           oninput={(e) => { scrim = +(e.currentTarget as HTMLInputElement).value; setBgScrim(scrim); }}
           aria-describedby={temFundoDeFoto ? undefined : `${uid}-foto`}
           aria-label={m.config_fundo_transparencia_detalhe()} />
    <em>{scrim}</em>
  </label>
  <!-- Solidez: a Transparência acima governa o painel; esta governa as CAIXAS de dentro dele
       (chip, campo, card, bloco de saída). Duas camadas, dois controles — no 0 as caixas somem no
       vidro e a tela vira uma superfície só; no 100 voltam a ser recorte chapado sobre a foto. -->
  <label class="bg-scrim" class:off={!temFundoDeFoto}>
    <span>{m.config_fundo_solidez()}</span>
    <input type="range" min="0" max="100" step="1"
           value={solidez}
           disabled={!temFundoDeFoto}
           oninput={(e) => { solidez = +(e.currentTarget as HTMLInputElement).value; setSurfaceSolid(solidez); }}
           aria-describedby={temFundoDeFoto ? undefined : `${uid}-foto`}
           aria-label={m.config_fundo_solidez_detalhe()} />
    <em>{solidez}</em>
  </label>
  {#if !temFundoDeFoto}
    <p class="bg-motivo" id="{uid}-foto">{m.config_aparencia_motivo_fundo_imagem()}</p>
  {/if}
  {#if pref === 'image' || temImagem}
    <div class="bg-img-row">
      <button class="bg-link" onclick={() => arquivoEl?.click()}>{m.config_fundo_trocar()}</button>
      {#if temImagem}<button class="bg-link danger" onclick={remover}>{m.config_fundo_remover()}</button>{/if}
    </div>
  {/if}
  {#if erro}<p class="bg-erro">⚠ {erro}</p>{/if}
</div>

<style>
  /* `min-width: 0` + `max-width` porque este bloco agora é sempre alto: os dois sliders deixaram de
     sumir sem foto de fundo, e na folha rápida de troca de sessão (linha flex de rótulo + controle)
     ele passava da borda — o motivo saía cortado e o rótulo "Fundo" ficava por baixo do slider. */
  .bg-wrap { display: flex; flex-direction: column; align-items: flex-end; gap: var(--space-1); min-width: 0; max-width: 100%; }
  .bg-file { display: none; }
  .bg-scrim { display: flex; align-items: center; gap: var(--space-2); width: 100%; }
  .bg-scrim span { color: var(--text-muted); font-size: var(--text-xs); white-space: nowrap; }
  .bg-scrim input { flex: 1; min-width: 60px; accent-color: var(--accent); }
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
  /* Controle apagado com o motivo ao lado: o texto fica visível, nunca só no `title`. */
  .bg-scrim.off { opacity: 0.45; }
  .bg-scrim input:disabled { cursor: default; }
  .bg-motivo { margin: 0; max-width: 100%; color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4; text-align: right; }

  .bg-toggle {
    display: inline-flex;
    flex-wrap: wrap;
    justify-content: flex-end;
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
  .bg-opt:disabled { opacity: 0.45; cursor: default; }
  .bg-opt:disabled:hover { color: var(--text-secondary); }
</style>

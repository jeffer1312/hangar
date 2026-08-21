<script lang="ts">
  import { getThemePref, setThemePref, getTextoDoDesktop, type ThemePref } from '../lib/theme';
  import { buscarPaleta, aplicarPaleta, limparPaleta, paletaEmCache } from '../lib/desktopTheme';
  import { aplicarCorTema } from '../lib/corTema';
  import * as m from '../paraglide/messages';

  let { onEscolha }: { onEscolha?: (p: ThemePref) => void } = $props();

  let pref = $state<ThemePref>(getThemePref());
  function pick(p: ThemePref) {
    pref = p;
    setThemePref(p);
    if (p === 'desktop') {
      buscarPaleta().then((pal) => { if (pal) aplicarPaleta(pal, getTextoDoDesktop()); });
    } else {
      limparPaleta();
      // O limparPaleta remove TODOS os tokens inline (a lista dele inclui --accent/--bg-*, que a cor
      // manual também escreve) — e ele rodou DEPOIS do applyTheme, que já tinha aplicado o desvio.
      // Sem reaplicar aqui, trocar de "Desktop" pra "Escuro" nascia sem a cor escolhida pelo usuário.
      aplicarCorTema();
    }
    onEscolha?.(p);
  }

  // "Desktop" so existe se HOUVER paleta pra valer — `ehLocal()` sozinho so descarta servidor
  // REMOTO; a pagina servida pela PROPRIA maquina do backend (Tailscale/VPS apontando pro backend
  // local, ou qualquer maquina sem o rice) tambem cai em `ehLocal()===true` e so descobre o 403/404
  // no clique, com a preferencia ja gravada e o botao "selecionado" sem nunca pintar nada. Por isso
  // o gate sonda a paleta de verdade, nao so a origem. Come do cache (Fix 3) quando ja se sabe a
  // resposta — so paga rede na primeira vez que esta folha abre.
  let temPaleta = $state(!!paletaEmCache());
  $effect(() => {
    if (paletaEmCache()) { temPaleta = true; return; }
    buscarPaleta().then((p) => { temPaleta = !!p; });
  });

  const opts = $derived<{ v: ThemePref; label: string; aria: string }[]>([
    { v: 'system', label: m.config_tema_auto(), aria: m.config_idioma_sistema() },
    { v: 'light', label: '☀', aria: m.config_tema_claro() },
    { v: 'dark', label: '☾', aria: m.config_tema_escuro() },
    ...(temPaleta
      ? [{ v: 'desktop' as ThemePref, label: m.config_aparencia_desktop(), aria: m.config_tema_desktop() }]
      : []),
  ]);
</script>

<div class="tt" role="group" aria-label={m.config_tema_curto()}>
  {#each opts as o (o.v)}
    <button
      class="tt-opt"
      class:active={pref === o.v}
      onclick={() => pick(o.v)}
      aria-pressed={pref === o.v}
      aria-label={o.aria}
    >{o.label}</button>
  {/each}
</div>

<style>
  .tt {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .tt-opt {
    min-height: 32px;
    min-width: 44px;
    padding: 0 var(--space-3);
    border-radius: 9px;
    font-size: var(--text-sm);
    color: var(--text-secondary);
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }
  .tt-opt.active {
    background: var(--accent-dim);
    color: var(--accent);
    font-weight: 600;
  }
</style>

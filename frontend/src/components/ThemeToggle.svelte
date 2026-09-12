<script lang="ts">
  import { getThemePref, setThemePref, getTextoDoDesktop, type ThemePref } from '../lib/theme';
  import { buscarPaleta, aplicarPaleta, limparPaleta, paletaEmCache } from '../lib/desktopTheme';
  import { aplicarCorTema } from '../lib/corTema';
  import * as m from '../paraglide/messages';

  let { onEscolha }: { onEscolha?: (p: ThemePref) => void } = $props();

  const uid = $props.id();

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
  // TRES estados, nao um booleano: "ainda nao sei" nao e "nao tem". Quem nao esta em Tema =
  // Desktop chega aqui com o cache vazio — `main.ts` so busca a paleta no boot se a preferencia
  // JA for 'desktop' —, entao a sondagem abaixo e sempre a primeira, e um booleano fazia a tela
  // afirmar "precisa de papel de parede" durante o voo, inclusive numa maquina que TEM.
  let sonda = $state<'indo' | 'sim' | 'nao'>(paletaEmCache() ? 'sim' : 'indo');
  $effect(() => {
    if (paletaEmCache()) { sonda = 'sim'; return; }
    let vivo = true;
    buscarPaleta().then((p) => { if (vivo) sonda = p ? 'sim' : 'nao'; });
    return () => { vivo = false; };
  });

  // A opcao EM USO nunca e apagada: `lib/theme.ts` devolve a preferencia gravada sem rebaixar
  // (diferente do fundo, que cai pra 'flat' sozinho), entao sem esse gate quem escolheu Desktop e
  // ficou sem paleta via a opcao marcada e apagada ao mesmo tempo, com um motivo negando a propria
  // selecao. Clicar nela re-sonda.
  const emUso = $derived(pref === 'desktop');
  const motivoVisivel = $derived(sonda === 'nao' && !emUso);

  // "Desktop" nao some quando nao da: fica na fileira, desabilitada, com o motivo escrito ao lado.
  // Some era pior que apagado — quem nunca viu a opcao nao tem como saber que ela existe nem o que
  // fazer pra ela voltar. Enquanto a sonda esta em voo ela fica apagada e CALADA: nao se clica no
  // que ainda nao se sabe, e nao se afirma o que ainda nao se sabe.
  const opts = $derived<{ v: ThemePref; label: string; aria: string; off: boolean }[]>([
    { v: 'system', label: m.config_tema_auto(), aria: m.config_idioma_sistema(), off: false },
    // Texto, não só o ícone: os vizinhos desta mesma fileira ("Automático", "Desktop") são
    // palavras, e o sol/lua sozinhos eram os únicos sem nada escrito.
    { v: 'light', label: m.config_tema_claro(), aria: m.config_tema_claro(), off: false },
    { v: 'dark', label: m.config_tema_escuro(), aria: m.config_tema_escuro(), off: false },
    { v: 'desktop', label: m.config_aparencia_desktop(), aria: m.config_tema_desktop(), off: sonda !== 'sim' && !emUso },
  ]);
</script>

<div class="tt-wrap">
  <div class="tt" role="group" aria-label={m.config_tema_curto()}>
    {#each opts as o (o.v)}
      <button
        class="tt-opt"
        class:active={pref === o.v}
        onclick={() => pick(o.v)}
        disabled={o.off}
        aria-pressed={pref === o.v}
        aria-label={o.aria}
        aria-describedby={o.v === 'desktop' && motivoVisivel ? `${uid}-tema` : undefined}
      >{o.label}</button>
    {/each}
  </div>
  {#if motivoVisivel}
    <!-- O motivo é irmão do grupo, então só o `aria-describedby` liga ele ao botão apagado: quem
         inspeciona o botão sozinho (rotor, exploração por gesto) ouviria "desabilitado" e nada mais.
         O id sai de `$props.id()` porque este componente é montado em vários lugares ao mesmo tempo
         (a tela de Aparência e uma folha de troca de sessão por painel do split): id fixo repetido
         faz o `aria-describedby` da segunda instância apontar para o parágrafo da primeira. -->
    <p class="tt-motivo" id="{uid}-tema">{m.config_aparencia_motivo_tema_desktop()}</p>
  {/if}
</div>

<style>
  .tt-wrap { display: inline-flex; flex-direction: column; align-items: flex-end; gap: 2px; }
  .tt-motivo {
    margin: 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.4;
    text-align: right;
  }
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
  .tt-opt:disabled { opacity: 0.45; cursor: default; }
</style>

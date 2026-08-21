<script lang="ts">
  // Seção "Cor do tema" da Aparência — destaque + tinta de fundo, por modo (escuro/claro).
  // Referência: painel "Customize Theme" do super.engineering (abas Light/Dark, canais
  // Background/Accent, "Copy from Dark"). Aqui: um segmentado escolhe QUAL modo se edita (nasce no
  // modo resolvido, porque é o que está na tela), swatches + cor personalizada pros dois canais, e
  // o slider de força só existe quando há tinta (sem tinta não há o que dosar).
  //
  // A lógica de persistência/aplicação mora em lib/corTema.ts; este componente é só o controle.
  // Não aparece no tema 'desktop' (gate no pai): lá a paleta Material You é a dona dos tokens.
  import SegmentedPicker from '../SegmentedPicker.svelte';
  import * as m from '../../paraglide/messages';
  import {
    getCorTema, setDestaque, setTinta, setForca, copiarDoOutroModo,
    type ModoCor, type CorTema,
  } from '../../lib/corTema';

  let { onMudanca }: { onMudanca?: () => void } = $props();

  // Modo em edição: o RESOLVIDO (data-theme), não a preferência — em "Sistema" é o que está na
  // tela, e editar o modo que não se vê parece bug. `modoInicial` é const (não-reativo) de
  // propósito: o $state nasce dele e as trocas posteriores relêem via trocarModo().
  const modoInicial: ModoCor =
    typeof document !== 'undefined' && document.documentElement.dataset.theme === 'light' ? 'light' : 'dark';
  let modo = $state<ModoCor>(modoInicial);
  let cor = $state<CorTema>(getCorTema(modoInicial));

  function trocarModo(v: ModoCor) {
    modo = v;
    cor = getCorTema(v);
  }
  function pickDestaque(hex: string | null) {
    setDestaque(modo, hex);
    cor = getCorTema(modo);
    onMudanca?.();
  }
  function pickTinta(hex: string | null) {
    setTinta(modo, hex);
    cor = getCorTema(modo);
    onMudanca?.();
  }
  function pickForca(n: number) {
    setForca(modo, n);
    cor = getCorTema(modo);
    onMudanca?.();
  }
  function copiar() {
    copiarDoOutroModo(modo);
    cor = getCorTema(modo);
    onMudanca?.();
  }

  // Accent de fábrica de cada modo (app.css) — primeiro swatch ("padrão") mostra a cor que o
  // usuário perdeu de vista, não um ∅ abstrato.
  const ACCENT_FABRICA: Record<ModoCor, string> = { dark: '#7c87e8', light: '#5b6ad0' };
  // Presets de destaque: o índigo de fábrica + as cores que o vídeo deles cicla (coral Claude,
  // verde, céu, fúcsia, âmbar). Todos passam no contraste de chip/texto dos dois modos.
  const SW_DESTAQUE = ['#d97757', '#34c759', '#0ea5e9', '#e879f9', '#f59e0b'];
  // Presets de tinta: verde/menta/azul/roxo/rosa — as janelas tingidas do vídeo.
  const SW_TINTA = ['#22c55e', '#14b8a6', '#3b82f6', '#8b5cf6', '#f43f5e'];
</script>

<div class="ct">
  <div class="ct-head">
    <SegmentedPicker
      value={modo}
      options={[
        { v: 'dark', label: m.config_tema_escuro(), aria: m.config_tema_escuro() },
        { v: 'light', label: m.config_tema_claro(), aria: m.config_tema_claro() },
      ]}
      ariaLabel={m.config_aparencia_cor_tema()}
      onPick={(v) => trocarModo(v as ModoCor)}
    />
    <button class="ct-copiar" onclick={copiar}>
      {modo === 'dark' ? m.config_aparencia_copiar_claro() : m.config_aparencia_copiar_escuro()}
    </button>
  </div>

  <div class="ct-linha">
    <span class="ct-nome">{m.config_aparencia_destaque()}</span>
    <div class="ct-swatches" role="group" aria-label={m.config_aparencia_destaque()}>
      <button
        class="ct-sw ct-sw--padrao" class:ativo={!cor.destaque}
        style="--sw:{ACCENT_FABRICA[modo]}"
        onclick={() => pickDestaque(null)}
        aria-label={`${m.config_aparencia_destaque()} — ${m.config_aparencia_voltar_padrao()}`}
        title={m.config_aparencia_voltar_padrao()}
      ></button>
      {#each SW_DESTAQUE as hex (hex)}
        <button
          class="ct-sw" class:ativo={cor.destaque === hex}
          style="--sw:{hex}"
          onclick={() => pickDestaque(cor.destaque === hex ? null : hex)}
          aria-label={hex} title={hex}
        ></button>
      {/each}
      <!-- Cor livre: o input color nativo abre o seletor do SO. Mostra a escolhida quando ela não
           é preset nenhum, senão um gradiente "qualquer cor". -->
      <label class="ct-sw ct-sw--custom" class:ativo={!!cor.destaque && !SW_DESTAQUE.includes(cor.destaque)}
             style="--sw:{cor.destaque ?? 'conic-gradient(#f43f5e, #f59e0b, #22c55e, #0ea5e9, #8b5cf6, #f43f5e)'}"
             title={m.config_aparencia_cor_custom()} aria-label={m.config_aparencia_cor_custom()}>
        <input type="color" value={cor.destaque ?? ACCENT_FABRICA[modo]}
               oninput={(e) => pickDestaque((e.currentTarget as HTMLInputElement).value)} />
      </label>
    </div>
  </div>

  <div class="ct-linha">
    <span class="ct-nome">{m.config_aparencia_tinta()}</span>
    <div class="ct-swatches" role="group" aria-label={m.config_aparencia_tinta()}>
      <button
        class="ct-sw ct-sw--nenhuma" class:ativo={!cor.tinta}
        onclick={() => pickTinta(null)}
        aria-label={m.lista_agrupar_nenhum()} title={m.lista_agrupar_nenhum()}
      >∅</button>
      {#each SW_TINTA as hex (hex)}
        <button
          class="ct-sw" class:ativo={cor.tinta === hex}
          style="--sw:{hex}"
          onclick={() => pickTinta(cor.tinta === hex ? null : hex)}
          aria-label={hex} title={hex}
        ></button>
      {/each}
      <label class="ct-sw ct-sw--custom" class:ativo={!!cor.tinta && !SW_TINTA.includes(cor.tinta)}
             style="--sw:{cor.tinta ?? 'conic-gradient(#f43f5e, #f59e0b, #22c55e, #0ea5e9, #8b5cf6, #f43f5e)'}"
             title={m.config_aparencia_cor_custom()} aria-label={m.config_aparencia_cor_custom()}>
        <input type="color" value={cor.tinta ?? '#3b82f6'}
               oninput={(e) => pickTinta((e.currentTarget as HTMLInputElement).value)} />
      </label>
    </div>
  </div>

  {#if cor.tinta}
    <label class="ap-slider ct-forca">
      <span>{m.config_aparencia_forca()}</span>
      <input type="range" min="5" max="100" step="1" value={cor.forca}
             oninput={(e) => pickForca(+(e.currentTarget as HTMLInputElement).value)} />
      <em>{cor.forca}</em>
    </label>
  {/if}
</div>

<style>
  .ct { display: flex; flex-direction: column; gap: var(--space-3); width: 100%; }
  .ct-head { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); }
  .ct-copiar {
    background: none; border: none; padding: 0;
    color: var(--accent); font-size: var(--text-xs); cursor: pointer;
  }
  .ct-copiar:hover { text-decoration: underline; }

  .ct-linha { display: flex; align-items: center; gap: var(--space-3); }
  .ct-nome { flex-shrink: 0; width: 92px; font-size: var(--text-xs); color: var(--text-secondary); }
  .ct-swatches { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }

  .ct-sw {
    width: 26px; height: 26px; border-radius: 50%;
    background: var(--sw);
    border: 2px solid transparent;
    box-shadow: inset 0 0 0 1px var(--border-default);
    cursor: pointer; padding: 0;
    display: inline-grid; place-items: center;
    color: var(--text-muted); font-size: 13px;
  }
  /* Ativo: anel na cor do accent + respiro do fundo entre anel e swatch, legível com qualquer cor. */
  .ct-sw.ativo { border-color: var(--accent); box-shadow: 0 0 0 2px var(--bg-base), 0 0 0 3px var(--accent); }
  .ct-sw--custom { overflow: hidden; position: relative; }
  .ct-sw--custom input {
    position: absolute; inset: 0; opacity: 0; width: 100%; height: 100%;
    cursor: pointer; padding: 0; border: none;
  }
  .ct-forca { padding-left: calc(92px + var(--space-3)); }
</style>

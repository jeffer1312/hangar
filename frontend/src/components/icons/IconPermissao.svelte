<script lang="ts">
  // Um ícone por modo de permissão. Pesquisa de 08/09/2026: NENHUM produto do ramo (Cursor,
  // Copilot, Claude Code, Warp, Zed, Cline, Codex) usa ícone por modo de autonomia — todos
  // resolvem com texto, porque quatro a seis níveis não se distinguem por forma. Aqui o ícone é
  // reforço do rótulo, que continua sempre à vista, e por isso cada modo ganha a metáfora do seu
  // EIXO: permissão e confiança. O `pause`/`fast-forward` de antes errava o eixo — é vocabulário
  // de tocador de mídia, que fala de tempo.
  //
  // Traçados do Lucide (lucide-static 1.42.0, ISC), copiados: seis desenhos fixos não pagam uma
  // dependência e um gerador. O material-icon-theme é dependência porque são centenas.
  let { modo, size = 16 }: { modo: string; size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if modo === 'plan'}
    <!-- compass: se orientar antes de andar; não mexe em nada -->
    <circle cx="12" cy="12" r="10" />
    <path d="m16.24 7.76-1.804 5.411a2 2 0 0 1-1.265 1.265L7.76 16.24l1.804-5.411a2 2 0 0 1 1.265-1.265z" />
  {:else if modo === 'manual'}
    <!-- hand: mão levantada, pare e pergunte -->
    <path d="M18 11V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2" />
    <path d="M14 10V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v2" />
    <path d="M10 10.5V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2v8" />
    <path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15" />
  {:else if modo === 'auto'}
    <!-- wand-sparkles: resolve o rotineiro sozinho -->
    <path d="m21.64 3.64-1.28-1.28a1.21 1.21 0 0 0-1.72 0L2.36 18.64a1.21 1.21 0 0 0 0 1.72l1.28 1.28a1.2 1.2 0 0 0 1.72 0L21.64 5.36a1.2 1.2 0 0 0 0-1.72" />
    <path d="m14 7 3 3" />
    <path d="M5 6v4" />
    <path d="M19 14v4" />
    <path d="M10 2v2" />
    <path d="M7 8H3" />
    <path d="M21 16h-4" />
    <path d="M11 3H9" />
  {:else if modo === 'acceptEdits'}
    <!-- file-check-2: o arquivo passa sozinho; o comando ainda não -->
    <path d="M10.5 22H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.706.706l3.588 3.588A2.4 2.4 0 0 1 20 8v6" />
    <path d="M14 2v5a1 1 0 0 0 1 1h5" />
    <path d="m14 20 2 2 4-4" />
  {:else if modo === 'bypassPermissions'}
    <!-- shield-off: a proteção desligada, oposto exato do shield-check -->
    <path d="m2 2 20 20" />
    <path d="M5 5a1 1 0 0 0-1 1v7c0 5 3.5 7.5 7.67 8.94a1 1 0 0 0 .67.01c2.35-.82 4.48-1.97 5.9-3.71" />
    <path d="M9.309 3.652A12.252 12.252 0 0 0 11.24 2.28a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1v7a9.784 9.784 0 0 1-.08 1.264" />
  {:else}
    <!-- flame: dontAsk e qualquer modo novo que o app ainda não conheça — sem volta -->
    <path d="M12 3q1 4 4 6.5t3 5.5a1 1 0 0 1-14 0 5 5 0 0 1 1-3 1 1 0 0 0 5 0c0-2-1.5-3-1.5-5q0-2 2.5-4" />
  {/if}
</svg>

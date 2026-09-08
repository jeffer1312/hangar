<script lang="ts">
  // Comandos (`/`): a barra dentro de uma tecla. O `/` solto era texto no meio de ícones — não
  // acompanhava peso nem tamanho dos vizinhos; a moldura o transforma no que ele é, uma tecla que
  // se aperta. Mesma família dos outros (24x24, traço 2, currentColor).
  let { size = 20 }: { size?: number } = $props();
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
  <rect x="3" y="3" width="18" height="18" rx="4" />
  <line x1="14" y1="8" x2="10" y2="16" />
</svg>

<script lang="ts">
  // Orquestração: um organograma — quem conduz em cima, quem executa embaixo. É o que a skill faz
  // (distribuir escopo entre sessões e cobrar de volta), e diz isso sem depender de metáfora
  // musical, que o 🎛 não entregava. Mesma família dos outros (24x24, traço 2, currentColor).
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
  <circle cx="12" cy="5" r="2.5" />
  <path d="M12 7.5v3M5 18v-4.5h14V18" />
  <circle cx="5" cy="19.5" r="2.5" />
  <circle cx="19" cy="19.5" r="2.5" />
</svg>

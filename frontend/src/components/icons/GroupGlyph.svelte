<script lang="ts">
  // Glifo de GRUPO de sessões (pareamento): dois robôs, um na frente com antena e olhos, outro
  // atrás só no contorno. Substitui o 🤝 — emoji de mãos dizia "pessoas", e o que se agrupa aqui
  // são agentes. Traço em currentColor, mesmo vocabulário dos outros glifos de linha do app.
  interface Props { size?: number }
  let { size = 14 }: Props = $props();
</script>

<svg class="gg" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  <path d="M13 12h4a3 3 0 0 1 3 3v2h-7" opacity="0.55" />
  <rect x="3" y="9" width="10" height="8" rx="2" />
  <path d="M8 9V6" />
  <circle cx="6.5" cy="13" r="1" fill="currentColor" stroke="none" />
  <circle cx="9.5" cy="13" r="1" fill="currentColor" stroke="none" />
</svg>

<style>
  .gg { flex-shrink: 0; vertical-align: -2px; }
</style>

<script lang="ts">
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
  <rect x="9" y="2" width="6" height="12" rx="3" />
  <path d="M5 10v2a7 7 0 0 0 14 0v-2" />
  <line x1="12" x2="12" y1="19" y2="22" />
</svg>

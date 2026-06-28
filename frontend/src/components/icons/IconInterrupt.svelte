<script lang="ts">
  let { size = 20 }: { size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="currentColor"
  aria-hidden="true"
>
  <rect x="6" y="6" width="12" height="12" rx="2.5" ry="2.5" />
</svg>

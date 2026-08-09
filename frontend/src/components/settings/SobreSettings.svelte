<script lang="ts">
  import HangarMark from '../icons/HangarMark.svelte';
  const REPO = 'https://github.com/jeffer1312/hangar';
</script>

<div class="about">
  <span class="mark" aria-hidden="true"><HangarMark size={56} /></span>
  <strong>Hangar</strong>
  <span class="version">{__HANGAR_VERSION__} · {__HANGAR_BUILD_DATE__}</span>
  <p>Dirija sessões Claude Code, Codex e Pi rodando no seu computador — do celular ou do desktop.</p>
  <a href={REPO} target="_blank" rel="noopener noreferrer">github.com/jeffer1312/hangar</a>
</div>

<style>
  .about { display: flex; flex-direction: column; align-items: center; text-align: center; gap: var(--space-2); padding: var(--space-6) var(--space-4); }
  .mark { color: var(--accent); }
  strong { font-size: var(--text-lg); color: var(--text-primary); }
  .version { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  p { margin: 0; max-width: 40ch; color: var(--text-secondary); font-size: var(--text-sm); }
  a { color: var(--accent); font-size: var(--text-sm); }
  a:hover { text-decoration: underline; }
</style>

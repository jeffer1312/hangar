<script lang="ts">
  interface Props {
    title?: string;
    showBack?: boolean;
    onBack?: () => void;
    onMenu?: () => void;
  }
  let { title = 'claude pocket', showBack = false, onBack, onMenu }: Props = $props();
</script>

<nav class="navbar">
  <div class="navbar-inner">
    {#if showBack}
      <button class="nav-btn back-btn" onclick={onBack} aria-label="Voltar">
        <svg width="10" height="17" viewBox="0 0 10 17" fill="none" aria-hidden="true">
          <path d="M9 1L1.5 8.5L9 16" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      </button>
    {:else}
      <div class="nav-spacer"></div>
    {/if}

    <span class="navbar-title">{title}</span>

    {#if onMenu}
      <button class="nav-btn menu-btn" onclick={onMenu} aria-label="Menu">
        <svg width="20" height="5" viewBox="0 0 20 5" fill="currentColor" aria-hidden="true">
          <circle cx="2.5" cy="2.5" r="2.5"/>
          <circle cx="10" cy="2.5" r="2.5"/>
          <circle cx="17.5" cy="2.5" r="2.5"/>
        </svg>
      </button>
    {:else}
      <div class="nav-spacer"></div>
    {/if}
  </div>
</nav>

<style>
  .navbar {
    background: var(--bg-base);
    border-bottom: 1px solid var(--border-subtle);
    padding-top: env(safe-area-inset-top);
    flex-shrink: 0;
    z-index: 20;
  }

  .navbar-inner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 48px;
    padding: 0 var(--space-4);
  }

  .navbar-title {
    font-size: var(--text-lg);
    font-weight: 500;
    color: var(--text-primary);
    letter-spacing: -0.01em;
    flex: 1;
    text-align: center;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .nav-btn {
    min-width: 44px;
    min-height: 44px;
    color: var(--accent);
    border-radius: var(--radius-md);
    transition: background 180ms ease-out;
    flex-shrink: 0;
  }

  .nav-btn:active {
    background: var(--bg-hover);
  }

  .nav-spacer {
    min-width: 44px;
  }
</style>

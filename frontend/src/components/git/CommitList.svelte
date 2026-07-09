<script lang="ts">
  import type { GitCommit } from '../../lib/api';

  interface Props {
    commits: GitCommit[];
    onSelect: (c: GitCommit | null) => void;
    selectedHash?: string;
    wtCount?: number;
  }
  let { commits, onSelect, selectedHash, wtCount = 0 }: Props = $props();

  // ── Grafo de commits: coluna de lanes (dots + linhas) a esquerda de cada commit na view de log.
  // Historico linear -> uma coluna so. Cap de lanes pra nao estourar o sheet estreito.
  const GRAPH_STEP = 14, GRAPH_R = 4.5, GRAPH_CAP = 5, GRAPH_H = 34;
  const graphLanes = $derived(Math.min(GRAPH_CAP, Math.max(1, ...commits.flatMap((c) => [
    (c.col ?? 0) + 1,
    ...(c.passthrough ?? []).map((p) => p + 1),
    ...(c.edges ?? []).map((e) => e.to_col + 1),
  ]))));
  const graphW = $derived(graphLanes * GRAPH_STEP + 4);
  const laneX = (col: number) => Math.min(col, GRAPH_CAP - 1) * GRAPH_STEP + GRAPH_R + 3;
  const laneColor = (col: number) => `hsl(${(col * 67) % 360} 55% 62%)`;   // cor estavel por lane
</script>

<div class="git-scroll git-log">
  {#if wtCount > 0}
    <button class="git-commit git-wt" class:sel={selectedHash === ''} onclick={() => onSelect(null)}>
      <span class="git-wt-dot" aria-hidden="true">●</span>
      <span class="git-c-sub">Working tree changes</span>
      <span class="git-c-when">{wtCount} arquivo{wtCount > 1 ? 's' : ''}</span>
    </button>
  {/if}
  {#each commits as c (c.hash)}
    {@const cx = laneX(c.col ?? 0)}
    <button class="git-commit" class:sel={selectedHash === c.hash} onclick={() => onSelect(c)} title={c.subject}>
      <svg class="git-graph" width={graphW} height={GRAPH_H} viewBox="0 0 {graphW} {GRAPH_H}" aria-hidden="true">
        <!-- lanes de OUTRAS branches que cruzam esta linha (vertical cheia, sem dot) -->
        {#each c.passthrough ?? [] as pc (pc)}
          {@const px = laneX(pc)}
          <line x1={px} y1="0" x2={px} y2={GRAPH_H} stroke={laneColor(pc)} stroke-width="2" />
        {/each}
        <!-- lane subindo (conecta ao commit acima na mesma coluna) -->
        <line x1={cx} y1="0" x2={cx} y2={GRAPH_H / 2} stroke={laneColor(c.col ?? 0)} stroke-width="2" />
        <!-- arestas descendo pros parents (merge = curva) -->
        {#each c.edges ?? [] as e, k (k)}
          {@const tx = laneX(e.to_col)}
          {#if e.curved && tx !== cx}
            <path d="M{cx} {GRAPH_H / 2} C {cx} {GRAPH_H - 6}, {tx} {GRAPH_H / 2 + 4}, {tx} {GRAPH_H}" stroke={laneColor(e.to_col)} stroke-width="2" fill="none" />
          {:else}
            <line x1={cx} y1={GRAPH_H / 2} x2={tx} y2={GRAPH_H} stroke={laneColor(e.to_col)} stroke-width="2" />
          {/if}
        {/each}
        <circle cx={cx} cy={GRAPH_H / 2} r={GRAPH_R} fill={laneColor(c.col ?? 0)} />
      </svg>
      <span class="git-c-hash">{c.short}</span>
      {#if c.refs}<span class="git-c-ref">{c.refs.split(', ')[0].replace('HEAD -> ', '')}</span>{/if}
      <span class="git-c-sub">{c.subject}</span>
      <span class="git-c-when">{c.rel}</span>
    </button>
  {/each}
  {#if !commits.length}<p class="git-muted">sem commits</p>{/if}
</div>

<style>
  .git-scroll {
    overflow-y: auto; max-height: 52vh; min-height: 0;
    overscroll-behavior: contain; -webkit-overflow-scrolling: touch;
    display: flex; flex-direction: column; gap: var(--space-1);
  }
  @media (min-width: 820px) { .git-scroll { max-height: 68vh; } }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }

  /* ── log: uma linha por commit (sem wrap; assunto com ellipsis) ── */
  .git-log { gap: 0; }
  .git-commit {
    display: flex; align-items: center; gap: var(--space-2); width: 100%;
    padding: 0 var(--space-2); min-height: 44px; border-radius: var(--radius-md);
    background: transparent; text-align: left; cursor: pointer;
  }
  .git-commit:active { background: var(--bg-elevated); }
  .git-commit.sel { background: var(--bg-elevated); }
  @media (hover: hover) { .git-commit:hover { background: var(--bg-hover); } }
  /* Coluna do grafo: dots + linhas de lane. overflow visible pra curva de merge nao cortar. */
  .git-graph { flex: 0 0 auto; overflow: visible; }
  /* Hash e o campo MENOS importante -> muted; o assunto lidera (primary), estilo GitLens/TortoiseGit. */
  .git-c-hash { flex: 0 0 auto; font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  .git-c-ref {
    flex: 0 1 auto; max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-size: 10px; padding: 0 6px; border-radius: var(--radius-full);
    background: var(--accent-dim); color: var(--accent); font-family: var(--font-mono);
  }
  .git-c-sub {
    flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-size: var(--text-sm); color: var(--text-primary);
  }
  .git-c-when { flex: 0 0 auto; font-size: var(--text-xs); color: var(--text-muted); white-space: nowrap; }

  /* linha sintetica "Working tree changes": dot em destaque, sem grafo (nao e um commit real). */
  .git-wt-dot { flex: 0 0 auto; font-size: 10px; color: var(--accent); }
</style>

<script lang="ts">
  // Diff do card de Edit/MultiEdit no estilo do Pi: old a esquerda (vermelho), new a direita
  // (verde), lado vazio hachurado. Fonte = old_string/new_string do tool_input (NUNCA o
  // tool_result) — entao o diff aparece no momento da chamada, antes do "Successfully replaced".
  // Estreito (celular) -> unificado de uma coluna, o mesmo dado em outra ordem.
  import { computeEditDiff, type EditDiff, type SplitRow } from '../lib/editdiff';
  import { highlightCodeLines, type DiffToken } from '../lib/highlight';

  interface Props {
    path: string;
    edits: { oldText: string; newText: string }[];
  }
  let { path, edits }: Props = $props();

  // Largura do COMPONENTE decide o layout (container, nao janela): no desktop o chat pode estar
  // espremido entre sidebar e painel de contexto, e a media query de 820px nao enxerga isso.
  // 600px = ~35 colunas de codigo por lado (o Pi desenha ~45) — apertado mas legivel, e o scroll
  // horizontal por lado cobre a linha comprida. Abaixo disso, unificado.
  const SPLIT_MIN = 600;
  let host: HTMLDivElement | undefined = $state();
  let split = $state(false);
  $effect(() => {
    if (!host) return;
    const ro = new ResizeObserver((es) => { split = (es[0]?.contentRect.width ?? 0) >= SPLIT_MIN; });
    ro.observe(host);
    return () => ro.disconnect();
  });

  interface RenderedEdit { diff: EditDiff; left: (DiffToken[] | null)[]; right: (DiffToken[] | null)[] }

  // Diff sincrono (barato); highlight async em cima (Shiki carrega grammar sob demanda).
  const diffs = $derived(edits.map((e) => computeEditDiff(e.oldText, e.newText)));
  let rendered = $state<RenderedEdit[]>([]);

  $effect(() => {
    const ds = diffs; // dependencia rastreada: recomputa quando edits mudam
    let vivo = true;
    const base: RenderedEdit[] = ds.map((diff) => ({
      diff,
      left: diff.rows.map(() => null),
      right: diff.rows.map(() => null),
    }));
    rendered = base;
    (async () => {
      for (let i = 0; i < ds.length; i++) {
        const rows = ds[i].rows;
        const [lt, rt] = await Promise.all([
          highlightCodeLines(rows.map((r) => r.left?.text ?? ''), path),
          highlightCodeLines(rows.map((r) => r.right?.text ?? ''), path),
        ]);
        if (!vivo) return;
        if (lt || rt) {
          rendered = rendered.map((r, j) => j !== i ? r : { ...r, left: lt ?? r.left, right: rt ?? r.right });
        }
      }
    })();
    return () => { vivo = false; };
  });

  // Numeros de linha sao monotonicos -> o maior e o da ULTIMA linha de cada edit. Sem spread de
  // Math.max nem flatMap: num diff gigante (o caso do fallback prefixo/sufixo) o spread estourava
  // RangeError e derrubava o card (achado do typescript-reviewer 2026-08-04).
  const gutWidth = $derived.by(() => {
    let max = 2;
    for (const d of diffs) {
      const last = d.rows[d.rows.length - 1];
      if (last) max = Math.max(max, last.left?.num ?? 0, last.right?.num ?? 0);
    }
    return max.toString().length;
  });

  const rowKind = (row: SplitRow, side: 'left' | 'right') => {
    const cell = row[side];
    if (!cell) return 'void';
    if (row.left && row.right) return row.left.text === row.right.text ? 'ctx' : side === 'left' ? 'del' : 'add';
    return side === 'left' ? 'del' : 'add';
  };
  const SIDES = ['left', 'right'] as const;

  // Path no cabecalho do diff: o card do Edit corta o meio com "…" (truncate CSS), entao o nome
  // do arquivo some exatamente quando ele e a parte que importa. Aqui o basename tem prioridade —
  // quem encolhe sob pressao e o diretorio (escolha do usuario 2026-08-04).
  const pathParts = $derived.by(() => {
    const i = path.replace(/\/+$/, '').lastIndexOf('/');
    return i < 0 ? { dir: '', base: path } : { dir: path.slice(0, i + 1), base: path.slice(i + 1) };
  });
</script>

<div class="ed" bind:this={host}>
  {#if path}
    <div class="ed-path" title={path}>
      {#if pathParts.dir}<span class="ed-dir">{pathParts.dir}</span>{/if}<span class="ed-base">{pathParts.base}</span>
    </div>
  {/if}
  {#each rendered as re, ei (ei)}
    {#if rendered.length > 1}
      <div class="ed-edit-head">Edição {ei + 1}/{rendered.length}</div>
    {/if}
    <div class="ed-stat">
      {#if re.diff.add || re.diff.del}
        <span class="stat-add">+{re.diff.add}</span> <span class="stat-del">−{re.diff.del}</span>
      {:else}
        <span class="stat-same">sem mudança de linhas</span>
      {/if}
    </div>

    {#if split}
      <div class="ed-split">
        {#each SIDES as side (side)}
          <pre class="ed-side" class:ed-side--new={side === 'right'}>{#each re.diff.rows as row, ri (ri)}{@const kind = rowKind(row, side)}{@const toks = (side === 'left' ? re.left : re.right)[ri]}<span
                class="ln"
                class:ctx={kind === 'ctx'}
                class:add={kind === 'add'}
                class:del={kind === 'del'}
                class:void={kind === 'void'}
              ><span class="sr-only">{kind === 'add' ? '+' : kind === 'del' ? '-' : ' '}</span><span class="gut" style:min-width="{gutWidth}ch">{row[side]?.num ?? ''}</span><span class="code">{#if toks}{#each toks as t, ti (ti)}<span style={t.color ? `color: ${t.color}` : undefined}>{t.content}</span>{/each}{/if}{#if !toks}{row[side]?.text ?? ''}{/if}</span></span>{/each}</pre>
        {/each}
      </div>
    {:else}
      <pre class="ed-uni">{#each re.diff.ops as op, oi (oi)}<span
            class="ln"
            class:add={op.op === 'add'}
            class:del={op.op === 'del'}
          ><span class="pfx">{op.op === 'add' ? '+' : op.op === 'del' ? '-' : ' '}</span>{op.text}</span>{/each}</pre>
    {/if}
  {/each}
</div>

<style>
  .ed { display: flex; flex-direction: column; gap: var(--space-2); min-width: 0; }

  /* Path do arquivo editado: diretorio encolhe com ellipsis, basename nunca corta. */
  .ed-path {
    display: flex; min-width: 0;
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.4;
  }
  .ed-dir {
    min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    color: var(--text-muted); opacity: 0.8;
  }
  .ed-base { flex-shrink: 0; color: var(--text-secondary); font-weight: 600; }

  .ed-edit-head {
    font-family: var(--font-mono); font-size: 10px; font-weight: 600;
    letter-spacing: 0.03em; text-transform: uppercase; color: var(--text-muted);
  }
  .ed-stat { font-family: var(--font-mono); font-size: var(--text-xs); }
  .ed-stat .stat-add { color: var(--success); }
  .ed-stat .stat-del { color: var(--error); }
  .ed-stat .stat-same { color: var(--text-muted); }

  /* Lado a lado: UM container com scroll vertical (alinha as duas metades de graca) e cada metade
     com o proprio scroll horizontal pra linha comprida nao esmagar a outra. */
  .ed-split {
    display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-inset);
    max-height: 46vh; overflow-y: auto; overscroll-behavior: contain;
  }
  .ed-side { margin: 0; padding: var(--space-2) 0; overflow-x: auto; }
  .ed-side--new { border-left: 1px solid var(--border-subtle); }

  .ed-uni {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    max-height: 46vh; overflow: auto; overscroll-behavior: contain;
  }

  .ed pre, .ed .ln {
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.5;
  }
  .ln { display: block; white-space: pre; }

  .gut {
    display: inline-block; padding: 0 var(--space-2) 0 var(--space-1);
    text-align: right; color: var(--text-muted); opacity: 0.7;
    user-select: none;
  }
  .add .gut { color: var(--success); opacity: 1; }
  .del .gut { color: var(--error); opacity: 1; }

  .ln.add { background: color-mix(in srgb, var(--success) 10%, transparent); }
  .ln.del { background: color-mix(in srgb, var(--error) 10%, transparent); }
  /* Lado vazio do pareamento: hachurado na diagonal, como o Pi desenha. */
  .ln.void {
    background: repeating-linear-gradient(
      -45deg,
      transparent 0 4px,
      color-mix(in srgb, var(--text-muted) 9%, transparent) 4px 5px
    );
  }

  .ed-uni .pfx { opacity: 0.7; user-select: none; }
  .ed-uni .add { color: var(--success); }
  .ed-uni .del { color: var(--error); }
</style>

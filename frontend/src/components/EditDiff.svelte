<script lang="ts">
  // Diff do card de Edit/MultiEdit no estilo do Pi: old a esquerda (vermelho), new a direita
  // (verde), lado vazio hachurado. Fonte = old_string/new_string do tool_input (NUNCA o
  // tool_result) — entao o diff aparece no momento da chamada, antes do "Successfully replaced".
  // Estreito (celular) -> unificado de uma coluna, o mesmo dado em outra ordem.
  import * as m from '../paraglide/messages';
  import { computeEditDiff, type EditDiff, type SplitRow } from '@hangar/core';
  import { highlightCodeLines, type DiffToken } from '../lib/highlight';
  import { toolLook } from '../lib/toolLook.svelte';
  import { rolagemSoAoClicar } from '../lib/rolagemSoAoClicar';

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
  // Totais do arquivo (soma das edições) — o chip da pele 'chips' mostra um número só por arquivo,
  // como a faixa do ToolGroup faz.
  const totalAdd = $derived(rendered.reduce((s, re) => s + re.diff.add, 0));
  const totalDel = $derived(rendered.reduce((s, re) => s + re.diff.del, 0));

  const pathParts = $derived.by(() => {
    const i = path.replace(/\/+$/, '').lastIndexOf('/');
    return i < 0 ? { dir: '', base: path } : { dir: path.slice(0, i + 1), base: path.slice(i + 1) };
  });
</script>

<div class="ed" bind:this={host}>
  {#if path}
    {#if toolLook.look === 'chips'}
      <!-- Pele 'chips': o caminho inteiro já está no chip da LINHA logo acima, então repeti-lo aqui
           era ruído. Vira o mesmo chip da faixa de arquivos — nome + contagem numa peça só. O
           caminho completo continua acessível no title. -->
      <div class="ed-chip-linha">
        <span class="ed-chip" title={path}>
          <span class="ed-chip-file">{pathParts.base}</span>
          {#if totalAdd}<span class="stat-add">+{totalAdd}</span>{/if}
          {#if totalDel}<span class="stat-del">−{totalDel}</span>{/if}
          {#if !totalAdd && !totalDel}<span class="stat-same">{m.editdiff_sem_mudanca()}</span>{/if}
        </span>
      </div>
    {:else}
      <div class="ed-path" title={path}>
        {#if pathParts.dir}<span class="ed-dir">{pathParts.dir}</span>{/if}<span class="ed-base">{pathParts.base}</span>
      </div>
    {/if}
  {/if}
  {#each rendered as re, ei (ei)}
    {#if rendered.length > 1}
      <div class="ed-edit-head">{m.editdiff_edicao({ n: ei + 1, total: rendered.length })}</div>
    {/if}
    <!-- Com UMA edição a contagem já está no chip acima; com várias, cada uma mostra a sua. -->
    {#if !(toolLook.look === 'chips' && rendered.length === 1 && path)}
      <div class="ed-stat">
        {#if re.diff.add || re.diff.del}
          <span class="stat-add">+{re.diff.add}</span> <span class="stat-del">−{re.diff.del}</span>
        {:else}
          <span class="stat-same">{m.editdiff_sem_mudanca()}</span>
        {/if}
      </div>
    {/if}

    {#if split}
      <div class="ed-split" use:rolagemSoAoClicar>
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
      <pre class="ed-uni" use:rolagemSoAoClicar>{#each re.diff.ops as op, oi (oi)}<span
            class="ln"
            class:add={op.op === 'add'}
            class:del={op.op === 'del'}
          ><span class="pfx">{op.op === 'add' ? '+' : op.op === 'del' ? '-' : ' '}</span>{op.text}</span>{/each}</pre>
    {/if}
  {/each}
</div>

<style>
  .ed { display: flex; flex-direction: column; gap: var(--space-2); min-width: 0; }

  /* Chip do arquivo (pele 'chips'): mesma peça da faixa do ToolGroup — mesma tinta, mesmo raio,
     mesma altura. É o que faz o cabeçalho do diff conversar com o resto em vez de ser outra coisa. */
  .ed-chip-linha { display: flex; }
  .ed-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    height: 28px;
    padding: 0 8px;
    border-radius: 6px;
    background: var(--fill-subtle);
    box-shadow: 0 0 0 1px var(--border-subtle), 0 1px 2px rgba(0, 0, 0, 0.18);
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text);
  }
  .ed-chip-file { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ed-chip .stat-add,
  .ed-chip .stat-del { flex-shrink: 0; font-variant-numeric: tabular-nums; }

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

  /* Lado a lado: UM container com scroll vertical (alinha as duas metades de graca). Sem scroll
     HORIZONTAL: linha comprida QUEBRA e fica inteira dentro da largura visivel. Arrastar pro lado
     pra ler e voltar pra continuar e pior que a linha ocupar duas alturas — e no lado a lado e
     duas vezes pior, porque as duas metades rolavam separadas e saiam de sincronia. */
  .ed-split {
    display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-inset);
    max-height: 46vh; overflow-y: auto; overscroll-behavior: contain;
  }
  .ed-side { margin: 0; padding: var(--space-2) 0; overflow-x: hidden; min-width: 0; }

  /* A linha vira flex pra o NUMERO ficar na coluna dele e o codigo quebrar so na sua caixa: com o
     numero no mesmo fluxo do texto, a continuacao da linha quebrada passava por baixo dele e a
     coluna de numeros deixava de ser coluna. */
  .ed-side .ln { display: flex; align-items: flex-start; }
  .ed-side .gut { flex: 0 0 auto; }
  .ed-side .code {
    flex: 1 1 auto; min-width: 0;
    /* `anywhere` e nao `break-word`: em codigo o comprido costuma ser um caminho ou uma string sem
       espaco nenhum, e break-word so quebra quando ja estourou a caixa. */
    white-space: pre-wrap; overflow-wrap: anywhere;
  }
  .ed-side--new { border-left: 1px solid var(--border-subtle); }

  /* Unificado (celular / coluna estreita): mesma regra — rola so na vertical, linha comprida quebra.
     Aqui a linha nao tem coluna de numero, so o prefixo +/-, entao basta a quebra no proprio bloco. */
  .ed-uni {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    max-height: 46vh; overflow-y: auto; overflow-x: hidden; overscroll-behavior: contain;
  }
  .ed-uni .ln { white-space: pre-wrap; overflow-wrap: anywhere; }

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

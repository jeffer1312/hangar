<script lang="ts">
  import * as m from '../../paraglide/messages';
  import type { TreeEntry } from '../../lib/types';

  interface Props {
    entries: TreeEntry[];
    abertos: Set<string>;
    selecionado: string | null;
    onToggle: (p: string) => void;
    onPick: (p: string) => void;
  }

  let { entries, abertos, selecionado, onToggle, onPick }: Props = $props();

  const RECUO = 14;   // px por nível, como no mock (docs/mocks/2026-08-15-arvore/base.css)
  const BASE = 8;     // recuo da raiz

  // Nível do nó = quantas barras o caminho tem (raiz = 0). O ARIA usa nível 1-based.
  const nivel = (p: string) => p.split('/').length - 1;

  // Letra da marca -> classe de cor. Rename/copy/unmerged/type-change não têm cor própria
  // no mock — caem no âmbar do M ("mudou, mas não é adição nem remoção").
  const classeMarca = (c: TreeEntry['changed']) =>
    c === 'A' ? 'm-A' : c === 'D' ? 'm-D' : c === '?' ? 'm-Q' : 'm-M';

  // Roving tabindex: a parada de Tab segue a seleção do pai; as setas movem o foco
  // (e a parada) sem depender do pai — é só foco, não seleção. Se o pai mudar a
  // seleção (clique em outra linha, troca de sessão), a parada volta a acompanhá-la.
  let ativa = $state<string | null>(null);
  $effect(() => { ativa = selecionado; });
  const linhaAtiva = $derived(
    entries.some((e) => e.path === ativa) ? ativa : (entries[0]?.path ?? null),
  );

  // Navegação por teclado: ↑/↓ andam entre as linhas, →/← abrem/fecham pasta, Enter
  // faz o mesmo que o clique (pasta alterna, arquivo escolhe).
  function teclado(e: KeyboardEvent) {
    const linhas = [...(e.currentTarget as HTMLElement).querySelectorAll<HTMLButtonElement>('.no')];
    const i = linhas.indexOf(document.activeElement as HTMLButtonElement);
    const atual = linhas[i]?.dataset.path;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      const alvo = linhas[Math.min(Math.max(i, -1) + 1, linhas.length - 1)];
      ativa = alvo?.dataset.path ?? null;
      alvo?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      const alvo = linhas[Math.max(i, 0) - 1];
      ativa = alvo?.dataset.path ?? null;
      alvo?.focus();
    } else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
      const ent = entries.find((en) => en.path === atual);
      if (ent?.is_dir) {
        const aberta = abertos.has(ent.path);
        if ((e.key === 'ArrowRight' && !aberta) || (e.key === 'ArrowLeft' && aberta)) {
          e.preventDefault();
          onToggle(ent.path);
        }
      }
    } else if (e.key === 'Enter') {
      const ent = entries.find((en) => en.path === atual);
      if (ent) {
        e.preventDefault();
        if (ent.is_dir) onToggle(ent.path); else onPick(ent.path);
      }
    }
  }
</script>

<!-- A árvore: uma linha por entrada, já na ordem que o pai mandar. O clique alterna
     pasta (onToggle) e escolhe arquivo (onPick), como no mock; Enter faz o mesmo. -->
<div class="arvore" role="tree" aria-label={m.arq_aba()} tabindex="-1" onkeydown={teclado}>
  {#each entries as ent (ent.path)}
    <button
      type="button"
      class="no {ent.is_dir ? 'pasta' : ''} {selecionado === ent.path ? 'sel' : ''}"
      style="padding-left: {BASE + nivel(ent.path) * RECUO}px"
      role="treeitem"
      aria-level={nivel(ent.path) + 1}
      aria-expanded={ent.is_dir ? (abertos.has(ent.path) ? 'true' : 'false') : undefined}
      aria-selected={selecionado === ent.path ? 'true' : 'false'}
      tabindex={ent.path === linhaAtiva ? 0 : -1}
      data-path={ent.path}
      onclick={() => (ent.is_dir ? onToggle(ent.path) : onPick(ent.path))}
    >
      <span class="chev" aria-hidden="true">{ent.is_dir ? (abertos.has(ent.path) ? '▾' : '▸') : ''}</span>
      <span class="ico" aria-hidden="true">
        {#if ent.is_dir}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>
        {:else}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>
        {/if}
      </span>
      <span class="nome">{ent.name}</span>
      {#if ent.add || ent.del}
        <span class="num"><span class="stat-add">+{ent.add}</span> <span class="stat-del">−{ent.del}</span></span>
      {:else}
        <span class="num"></span>
      {/if}
      {#if ent.changed}
        <span class="marca {classeMarca(ent.changed)}">{ent.changed}</span>
      {:else}
        <span class="marca"></span>
      {/if}
    </button>
  {/each}
</div>

<style>
  /* Mesmas regras do mock (docs/mocks/2026-08-15-arvore/base.css) — a barra é o HTML. */
  .arvore { padding: 2px 0 10px; overflow-y: auto; }
  .no {
    display: flex;
    align-items: center;
    gap: 5px;
    /* 4px (nao 3): com o alvo global de 44px sobrescrito abaixo, 3px cairia pra 23,6px —
       abaixo do minimo de 24px (WCAG 2.5.8 AA). 4px da 25,5px, a densidade da aba irma
       (linha de Task do plano: ~24,6px) e a do mock (24px). */
    padding: 4px 10px 4px 0;
    cursor: pointer;
    font-size: 13px;
    line-height: 1.35;
    color: var(--text-secondary);
    border: 0;
    background: none;
    width: 100%;
    text-align: left;
    font-family: inherit;
    /* Sobrescreve o alvo global de 44px (app.css) — a arvore e lista densa por desenho;
       o Composer.svelte tem o mesmo remedio pro pill compacto. */
    min-height: 0;
    min-width: 0;
  }
  .no:hover { background: var(--bg-hover); }
  .no:focus-visible { outline: 1px solid var(--accent); outline-offset: -1px; }
  .no.sel { background: var(--accent-dim); color: var(--text-primary); }
  .no .chev {
    width: 12px;
    flex: none;
    color: var(--text-muted);
    font-size: 9px;
    text-align: center;
  }
  .no .ico { width: 14px; flex: none; color: var(--text-muted); display: grid; place-items: center; }
  .no .ico svg { width: 13px; height: 13px; }
  .no.pasta > .nome { color: var(--text-primary); }
  .no .nome {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* O par +N −M por linha, como no Paseo. Fica ANTES da marca, e some no arquivo intocado. */
  .num {
    flex: none;
    font-family: var(--font-mono);
    font-size: 10.5px;
    letter-spacing: -0.2px;
    white-space: nowrap;
    opacity: 0.9;
  }
  .num .stat-add { color: var(--success); }
  .num .stat-del { color: var(--error); }

  /* A marca fica na EXTREMA direita e sobe pela árvore (a pasta herda do descendente). */
  .marca {
    flex: none;
    font-size: 10.5px;
    font-weight: 600;
    width: 12px;
    text-align: center;
    font-family: var(--font-mono);
  }
  .m-M { color: var(--warning); }
  .m-A { color: var(--success); }
  .m-D { color: var(--error); }
  .m-Q { color: var(--text-muted); }
</style>

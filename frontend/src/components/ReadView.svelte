<script lang="ts">
  // Resultado da ferramenta Read com highlight de codigo (Shiki), no lugar do <pre> cru.
  // Dois formatos de entrada: Claude devolve "N\tconteudo" (numero + tab), Pi devolve o conteudo
  // como esta (sem numeros). Numerado so e assumido se QUASE TODA linha casa "^\d+\t" — um tab
  // solto em dado (saida de grep, TSV) nao pode virar gutter falso. Recolhido/expandido quem
  // controla e o ToolCard; aqui e so a pintura.
  import { highlightCodeLines, type DiffToken } from '../lib/highlight';

  interface Props {
    path: string;
    text: string;
  }
  let { path, text }: Props = $props();

  interface Ln { num: number | null; content: string }

  const lines = $derived.by((): Ln[] => {
    const raw = text.replace(/\n$/, '').split('\n');
    const NUM = /^(\d+)\t(.*)$/;
    const matches = raw.filter((l) => NUM.test(l)).length;
    // Quase toda linha numerada -> formato cat -n do Claude. Abaixo disso, conteudo cru (Pi).
    if (raw.length > 0 && matches / raw.length >= 0.8) {
      return raw.map((l) => {
        const m = l.match(NUM);
        return m ? { num: parseInt(m[1], 10), content: m[2] } : { num: null, content: l };
      });
    }
    return raw.map((l) => ({ num: null, content: l }));
  });

  const gutWidth = $derived(
    Math.max(2, ...lines.filter((l) => l.num !== null).slice(-1).map((l) => String(l.num).length), 0)
  );

  let tokens = $state<(DiffToken[] | null)[]>([]);
  $effect(() => {
    const ls = lines;
    tokens = ls.map(() => null);
    let vivo = true;
    (async () => {
      const t = await highlightCodeLines(ls.map((l) => l.content), path);
      if (vivo && t) tokens = t;
    })();
    return () => { vivo = false; };
  });
</script>

<pre class="rv" style:--gut="{gutWidth + 1.5}ch">{#each lines as ln, i (i)}{@const toks = tokens[i]}<span
      class="ln"
    >{#if ln.num !== null}<span class="gut">{ln.num}</span>{/if}<span class="code">{#if toks}{#each toks as t, ti (ti)}<span style={t.color ? `color: ${t.color}` : undefined}>{t.content}</span>{/each}{/if}{#if !toks}{ln.content}{/if}</span></span>{/each}</pre>

<style>
  .rv {
    margin: 0;
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.5;
  }
  /* pre-wrap: o telefone nao tem largura pra scroll horizontal em leitura longa. O gutter fica
     pendurado na margem (margin-left negativo), entao a linha QUEBRADA indenta atras do numero. */
  .ln { display: block; white-space: pre-wrap; word-break: break-word; padding-left: var(--gut); }
  .gut {
    display: inline-block; width: var(--gut); margin-left: calc(-1 * var(--gut));
    padding-right: 1ch;
    text-align: right; color: var(--text-muted); opacity: 0.7;
    user-select: none;
  }
  .code { color: var(--text-secondary); }
</style>

<script lang="ts">
  // Ícone da chamada de ferramenta na pele 'chips'. Quatro famílias, como no beautiful-ui —
  // pensar / escrever / rodar / ler —, resolvidas pelo NOME da tool. Desconhecida cai em 'run',
  // que é o verbo mais genérico ("fez alguma coisa") e nunca some da linha.
  //
  // SVG inline em vez de biblioteca de ícone: o app não tem nenhuma e não vale ganhar uma por
  // quatro desenhos. currentColor em tudo, então quem pinta é o CSS de quem usa (a fase da
  // chamada colore o ícone: accent rodando, erro em vermelho).
  interface Props {
    /** Nome da tool como vem do transcript (Bash, Read, Edit, Write, MultiEdit, …). */
    tool?: string | null;
    size?: number;
  }
  let { tool = null, size = 13 }: Props = $props();

  type Familia = 'think' | 'write' | 'run' | 'read';

  const familia = $derived.by<Familia>(() => {
    const t = (tool ?? '').toLowerCase();
    if (t === 'read' || t === 'notebookread' || t === 'glob' || t === 'grep') return 'read';
    if (t === 'edit' || t === 'write' || t === 'multiedit' || t === 'notebookedit') return 'write';
    if (t === 'task' || t === 'todowrite' || t === 'exitplanmode' || t === 'askuserquestion') return 'think';
    return 'run';
  });
</script>

<svg
  width={size} height={size} viewBox="0 0 24 24" aria-hidden="true"
  fill={familia === 'think' ? 'currentColor' : 'none'}
  stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
>
  {#if familia === 'think'}
    <path d="M12 2l2.4 7.2L22 12l-7.6 2.8L12 22l-2.4-7.2L2 12l7.6-2.8z" stroke="none" />
  {:else if familia === 'write'}
    <path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z" />
  {:else if familia === 'read'}
    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
    <path d="M14 2v6h6" />
  {:else}
    <path d="M4 17l6-5-6-5M12 19h8" />
  {/if}
</svg>

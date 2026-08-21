<script lang="ts">
  // Lista de tarefas do agente como CÁPSULAS, portada do beautiful-ui (Task Rows).
  //
  // A lista não vem pronta de nenhum evento: o TaskCreate/TaskUpdate é incremental e o id nasce no
  // texto do resultado — quem reconstrói é o `foldTasks` (lib/tasks.ts, com teste). Aqui é só
  // desenho.
  //
  // Medidas do original (computed style): cápsula de 44px, respiro 10px, gap 10px entre os itens,
  // raio 22px fechada e 14px aberta, distintivo de 24px, rótulo 13px/500, etiqueta 22px.
  import type { Task } from '@hangar/core';
  import * as m from '../paraglide/messages';

  interface Props {
    tasks: Task[];
    /** false = histórico remontado (paginação): entra parado, sem escalonar a animação. */
    animate?: boolean;
  }
  let { tasks, animate = true }: Props = $props();

  // Aberta por toque, uma de cada vez não — várias podem ficar abertas, como no original.
  let abertas = $state<Record<string, boolean>>({});
  const chave = (t: Task, i: number) => t.id || `novo-${i}`;

  // Número mostrado dentro do anel: a POSIÇÃO na lista, como no original (1, 2, 3…), não o id do
  // Claude Code — o id salta (#7, #12) e não diz nada pra quem olha.
  function alternar(k: string) {
    abertas[k] = !abertas[k];
  }
</script>

<div class="tr-lista">
  {#each tasks as t, i (chave(t, i))}
    {@const k = chave(t, i)}
    {@const aberta = !!abertas[k]}
    <div
      class="tr-cap"
      class:noanim={!animate}
      class:aberta
      style:animation-delay={animate ? `${i * 80}ms` : undefined}
    >
      <button
        type="button"
        class="tr-btn"
        aria-expanded={aberta}
        onclick={() => alternar(k)}
      >
        <span class="tr-marca">
          {#if t.status === 'completed'}
            <span class="tr-selo tr-selo--ok" aria-label={m.tasks_concluida()}>
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="M20 6L9 17l-5-5" />
              </svg>
            </span>
          {:else}
            <!-- Anel com o número: girando quando está em andamento, parado quando é fila. -->
            <span class="tr-anel" class:girando={t.status === 'in_progress'}>
              <svg width="24" height="24" viewBox="0 0 24 24" aria-hidden="true">
                <circle cx="12" cy="12" r="11" fill="none" stroke="var(--border-default)" stroke-width="2" />
                {#if t.status === 'in_progress'}
                  <circle cx="12" cy="12" r="11" fill="none" stroke="var(--accent)" stroke-width="2"
                          stroke-linecap="round" stroke-dasharray="19 50" />
                {/if}
              </svg>
              <span class="tr-num">{i + 1}</span>
            </span>
          {/if}
        </span>

        <span class="tr-titulo">
          {t.status === 'in_progress' && t.activeForm ? t.activeForm : t.subject}
        </span>

        {#if t.status === 'completed'}
          <span class="tr-etiqueta">{m.tasks_concluida()}</span>
        {/if}

        <span class="tr-chevron" class:open={aberta} aria-hidden="true">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6" /></svg>
        </span>
      </button>

      <!-- Mesma gramática de expandir do resto: grid 0fr -> 1fr, sem medir altura no JS. -->
      <div class="tr-wrap" style:grid-template-rows={aberta ? '1fr' : '0fr'} style:opacity={aberta ? 1 : 0}>
        <div class="tr-clip">
          <div class="tr-detalhe">{t.description || m.tasks_sem_descricao()}</div>
        </div>
      </div>
    </div>
  {/each}
</div>

<style>
  .tr-lista {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: var(--space-2) 0;
  }

  /* Superfície pelos tokens (--surface-raised), não --bg-* cru: com papel de parede a cápsula
     acompanha o slider de Transparência em vez de virar bloco chapado. */
  .tr-cap {
    overflow: hidden;
    background: var(--surface-raised);
    box-shadow: 0 0 0 1px var(--border-subtle), 0 1px 2px rgba(0, 0, 0, 0.18);
    border-radius: 22px;
    transition: border-radius 300ms var(--ease-out);
    animation: fade-up 450ms cubic-bezier(0.23, 1, 0.32, 1) both;
  }
  .tr-cap.aberta { border-radius: 14px; }
  .tr-cap.noanim { animation: none; }

  .tr-btn {
    display: flex;
    align-items: center;
    justify-content: flex-start;   /* o app tem button { justify-content: center } global */
    gap: 10px;
    width: 100%;
    height: 44px;
    padding: 0 10px;
    border: none;
    background: transparent;
    text-align: left;
    cursor: pointer;
    transition: background-color 100ms var(--ease-out);
  }
  .tr-btn:hover { background: var(--fill-subtle); }

  .tr-marca { display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0; width: 24px; height: 24px; }

  .tr-selo {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    color: #fff;
    animation: pop-in 300ms cubic-bezier(0.23, 1, 0.32, 1) both;
  }
  .tr-selo--ok { background: var(--success); }

  .tr-anel { position: relative; display: inline-flex; align-items: center; justify-content: center; width: 24px; height: 24px; }
  .tr-anel.girando svg { animation: spin 1.1s linear infinite; }
  .tr-anel svg { position: absolute; inset: 0; }
  .tr-num { position: relative; font-size: 10.5px; font-weight: 600; font-variant-numeric: tabular-nums; color: var(--text); }

  .tr-titulo {
    flex: 1 1 auto;
    min-width: 0;
    font-size: 13px;
    font-weight: 500;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Etiqueta em tinta da própria cor, não fundo opaco — mesma ideia do --fill-subtle. */
  .tr-etiqueta {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    height: 22px;
    padding: 0 8px;
    border-radius: var(--radius-full);
    background: color-mix(in srgb, var(--success) 16%, transparent);
    color: var(--success);
    font-size: 11.5px;
    font-weight: 500;
  }

  .tr-chevron {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    margin-left: -8px;
    color: var(--text-muted);
    transition: transform 300ms var(--ease-out);
  }
  .tr-chevron.open { transform: rotate(180deg); }

  .tr-wrap { display: grid; transition: grid-template-rows 300ms cubic-bezier(0.23, 1, 0.32, 1), opacity 300ms var(--ease-out); }
  .tr-clip { min-height: 0; overflow: hidden; }
  .tr-detalhe {
    padding: 0 12px 12px 44px;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--text-secondary);
  }
</style>

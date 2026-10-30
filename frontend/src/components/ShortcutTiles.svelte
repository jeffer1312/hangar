<script lang="ts">
  // Seção "Ações": os atalhos customizados como blocos (ícone em cima, rótulo embaixo), no desenho do
  // painel direito do app nativo. Grade que quebra linha em vez de rolar: com mais largura cabem mais
  // colunas, e cada bloco divide a linha por igual. Usada no painel do desktop e no "⋯" do celular.
  import ShortcutIcon from './icons/ShortcutIcon.svelte';
  import { shortcutMissingSecret, runsInHangar, hangarKeyOf, type LiveShortcutTerminal, type ShortcutSendText, type ShortcutShell } from '@hangar/core';
  import { onDestroy, type Snippet } from 'svelte';
  import type { CustomScoped } from '../lib/shortcuts.svelte';
  import { tileStateOf, runningFor } from '../lib/hangarTerminals.svelte';
  import * as m from '../paraglide/messages';

  interface Props {
    shortcuts: CustomScoped[];
    onShortcut: (s: ShortcutSendText | ShortcutShell) => void;
    onAdd?: () => void;
    // Controles extras do cabeçalho (menu de importar/exportar), à direita do "+".
    extra?: Snippet;
    // Nome do projeto da sessão (dica da marca) e erro ao ler os atalhos dele (uma linha discreta).
    projectName?: string;
    projectError?: string;
    // Chave do repositório (identidade da cópia No Hangar de atalho do projeto) e a sessão desta tela.
    projectKey?: string;
    sessionName?: string;
    // Terminal No Hangar do atalho (uma cópia por servidor) e o mais novo dele nesta sessão.
    hangarOf?: (key: string) => LiveShortcutTerminal | null;
    sessionTerminal?: (key: string) => LiveShortcutTerminal | null;
  }
  let { shortcuts, onShortcut, onAdd, extra, projectName = '', projectError = '', projectKey, sessionName,
    hangarOf, sessionTerminal }: Props = $props();

  // Relógio do "rodando · X min": sem ele o texto congela no primeiro valor.
  let now = $state(Date.now());
  const clock = setInterval(() => (now = Date.now()), 30_000);
  onDestroy(() => clearInterval(clock));

  // Terminais No Hangar abertos por OUTRA sessão: uma nota só, uma linha por terminal.
  const elsewhere = $derived(shortcuts.flatMap(({ shortcut: s, scope }) => {
    const term = runsInHangar(s) ? hangarOf?.(hangarKeyOf(scope, s.id, projectKey)) ?? null : null;
    return term?.alive && term.origin && term.origin !== sessionName
      ? [{ key: scope + s.id, label: s.label, origin: term.origin }] : [];
  }));

  function elapsed(created: number) {
    const r = runningFor(created, now);
    return 'minutes' in r ? m.hangar_min({ n: r.minutes }) : m.hangar_h({ n: r.hours });
  }
</script>

<section class="acoes" aria-label={m.ctx_acoes()}>
  <div class="acoes-topo">
    <span class="acoes-titulo">{m.ctx_acoes()}</span>
    <span class="acoes-ctl">
      {@render extra?.()}
      {#if onAdd}
        <button type="button" class="acoes-add" onclick={onAdd} aria-label={m.atalhos_add()} title={m.atalhos_add()}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
               stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
        </button>
      {/if}
    </span>
  </div>
  <div class="acoes-grade">
    {#each shortcuts as { shortcut: s, scope, key } (key)}
      {@const falta = shortcutMissingSecret(s)}
      {@const rotulo = scope === 'project' ? m.atalhos_projeto_marca({ rotulo: s.label, nome: projectName }) : s.label}
      {@const tileKey = hangarKeyOf(scope, s.id, projectKey)}
      {@const hangarTerm = runsInHangar(s) ? hangarOf?.(tileKey) ?? null : null}
      {@const sessionTerm = s.type === 'shell' && !runsInHangar(s) ? sessionTerminal?.(tileKey) ?? null : null}
      <!-- O da sessão que saiu não acende nem marca "caiu": a saída continua na aba do painel. -->
      {@const tileState = sessionTerm ? (sessionTerm.alive ? tileStateOf(sessionTerm) : 'idle') : tileStateOf(hangarTerm)}
      {@const liveTerm = sessionTerm ?? hangarTerm}
      {@const stateLine = tileState === 'running' && liveTerm?.created ? m.hangar_rodando_ha({ tempo: elapsed(liveTerm.created) })
        : tileState === 'asking' ? m.atalho_tile_pergunta()
        : tileState === 'exited' && hangarTerm ? m.atalho_tile_caiu({ codigo: String(hangarTerm.exit_code ?? '?') }) : ''}
      {@const tooltip = hangarTerm?.alive && hangarTerm.origin && hangarTerm.origin !== sessionName
        ? m.atalho_tile_dica_hangar({ rotulo: s.label, sessao: hangarTerm.origin })
        : tileState === 'running' ? m.atalho_tile_parar({ rotulo: s.label }) : rotulo}
      <!-- Credencial em branco (veio de uma importação): o bloco fica apagado e o clique avisa. -->
      <button type="button" class="acao-bloco" class:pendente={!!falta} class:hg={runsInHangar(s)} class:rodando={tileState === 'running'}
              class:pergunta={tileState === 'asking'} onclick={() => onShortcut(s)}
              aria-label={stateLine ? `${rotulo} · ${stateLine}` : rotulo}
              title={falta ? m.atalhos_segredo_falta({ nome: falta }) : tooltip}>
        {#if scope === 'project'}<span class="acao-projeto" aria-hidden="true"></span>{/if}
        {#if runsInHangar(s)}
          <span class="acao-hangar" aria-hidden="true">
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></svg>{m.term_grupo_hangar()}
          </span>
        {/if}
        <ShortcutIcon icon={s.icon} />
        <span class="acao-rotulo">{s.label}</span>
        {#if stateLine}<span class="acao-estado">{stateLine}</span>{/if}
      </button>
    {/each}
  </div>
  <!-- Vale também no celular, onde o title não aparece. -->
  {#if elsewhere.length}
    <div class="acoes-nota">
      {#each elsewhere as n (n.key)}
        <span><b>{n.label}</b> {m.atalho_tile_nota_hangar({ sessao: n.origin })}</span>
      {/each}
    </div>
  {/if}
  {#if projectError}
    <p class="acoes-erro" title={projectError}>{m.atalhos_projeto_erro_fileira({ msg: projectError })}</p>
  {/if}
</section>

<style>
  .acoes { display: flex; flex-direction: column; gap: var(--space-2); }
  .acoes-topo { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); min-height: 24px; }
  /* Mesma receita dos rótulos de seção do painel (tokens de app.css). */
  .acoes-titulo {
    color: var(--text-muted); font-size: var(--label-size); font-weight: var(--label-weight);
    letter-spacing: var(--label-tracking); text-transform: uppercase;
  }
  .acoes-ctl { position: relative; display: inline-flex; align-items: center; gap: 2px; }
  .acoes-add {
    display: inline-flex; align-items: center; justify-content: center; width: 24px; height: 24px;
    border: 0; border-radius: var(--radius-sm); background: transparent; color: var(--text-muted); cursor: pointer;
  }
  .acoes-add:hover { background: var(--surface-raised); color: var(--text-primary); }
  .acoes-add:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  /* auto-fit + minmax: as colunas saem da largura e as vazias somem, então poucos blocos dividem a
     linha inteira em vez de ficarem encostados à esquerda. */
  .acoes { container-type: inline-size; }
  /* No máximo cinco por linha: o piso da coluna é o maior entre 88px e um quinto da linha. */
  .acoes-grade { display: grid; grid-template-columns: repeat(auto-fit, minmax(max(88px, calc((100% - 24px) / 5)), 1fr)); gap: 6px; }
  @container (max-width: 200px) { .acoes-grade { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
  .acao-bloco {
    position: relative; min-width: 0; min-height: 58px; display: flex; flex-direction: column; align-items: center; justify-content: center;
    gap: 4px; padding: 8px 4px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--surface-raised) 50%, transparent); color: var(--text-primary); cursor: pointer;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }
  .acao-bloco:hover { background: var(--surface-raised); color: var(--text-primary); }
  .acao-bloco:active { background: var(--bg-hover); }
  .acao-bloco.pendente { opacity: 0.55; border-style: dashed; }
  .acao-bloco:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .acao-bloco :global(svg) { flex-shrink: 0; width: 18px; height: 18px; }
  /* Marca de "deste projeto": ponto no canto, na cor de destaque apagada. */
  .acao-projeto {
    position: absolute; top: 5px; right: 5px; width: 6px; height: 6px;
    border-radius: var(--radius-full); background: var(--accent); opacity: 0.7;
  }
  .acoes-nota { display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--surface-raised) 50%, transparent);
    font-size: var(--text-xs); line-height: 1.5; color: var(--text-secondary); }
  .acoes-nota b { font-weight: 600; color: var(--success-text); }
  .acao-bloco.rodando { gap: 6px; border-color: color-mix(in srgb, var(--success) 45%, transparent); background: color-mix(in srgb, var(--success) 8%, transparent); }
  .acao-bloco.pergunta { gap: 6px; border-color: color-mix(in srgb, var(--warning) 55%, transparent); background: color-mix(in srgb, var(--warning) 8%, transparent); }
  /* Marca no canto de cima; o tile com marca ganha respiro em cima pra ela não cobrir o ícone. */
  .acao-bloco.hg { padding-top: 22px; }
  .acao-hangar { position: absolute; top: 8px; right: 8px; display: flex; align-items: center; gap: 4px;
    font-size: var(--text-3xs); letter-spacing: 0.04em; color: var(--text-muted); }
  .acao-bloco .acao-hangar :global(svg) { width: 11px; height: 11px; }
  .acao-projeto ~ .acao-hangar { right: 20px; }
  .acao-bloco.rodando .acao-hangar { color: var(--success-text); }
  .acao-bloco.pergunta .acao-hangar { color: var(--warning-text); }
  .acao-estado { display: flex; align-items: center; gap: 5px; font-size: var(--text-2xs); color: var(--text-muted); }
  .acao-bloco.rodando .acao-estado { color: var(--success-text); }
  .acao-bloco.rodando .acao-estado::before { content: ''; width: 6px; height: 6px; flex-shrink: 0; border-radius: 50%; background: var(--success); }
  .acao-bloco.pergunta .acao-estado { color: var(--warning-text); }
  .acoes-erro {
    margin: 0; font-size: var(--text-xs); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  /* Duas linhas antes de cortar: rótulo curto demais escondia o que o atalho faz. */
  .acao-rotulo {
    max-width: 100%; font-size: 13px; font-weight: 400; line-height: 1.25; text-align: center;
    overflow: hidden; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical;
    overflow-wrap: anywhere;
  }
</style>

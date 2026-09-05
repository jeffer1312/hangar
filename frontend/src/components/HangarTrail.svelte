<script lang="ts">
  import * as m from '../paraglide/messages';
  import type { AcaoHangar } from '../lib/hangarCmd';

  // Rajada de comandos do hangar numa trilha só. O caso é o do orquestrador: ele lista as sessões,
  // cria uma, pareia e manda o recado de largada em segundos — quatro cartões soltos contam a
  // mesma história pior do que quatro linhas em ordem.
  export type PassoHangar = { acao: AcaoHangar; hora: string | null };

  interface Props {
    passos: PassoHangar[];
    /** Rodou tudo em quanto tempo (ms). */
    total?: number | null;
  }
  let { passos, total = null }: Props = $props();

  function titulo(a: AcaoHangar): string {
    if (a.erro) return m.hangar_trilha_falhou();
    switch (a.verbo) {
      case 'criar': return m.hangar_trilha_criou({ nome: a.alvo ?? '?' });
      case 'listar': return a.sessoes?.length
        ? m.hangar_cmd_listou({ n: a.sessoes.length })
        : m.hangar_cmd_listou_sem_saida();
      case 'recado': return m.hangar_cmd_recado({ nome: a.alvo ?? '?' });
      case 'parear': return m.hangar_cmd_pareou({ nome: a.alvo ?? '?' });
      case 'desparear': return m.hangar_cmd_desparaeu();
      case 'grupo': return m.hangar_cmd_grupo({ n: a.peers?.length ?? 0 });
    }
  }

  function detalhe(a: AcaoHangar): string {
    if (a.erro) return a.erro.split('\n')[0];
    if (a.verbo === 'criar') return [a.provider, a.motor, a.worktree ? 'worktree' : null, a.cwd].filter(Boolean).join(' · ');
    if (a.texto) return a.texto;
    if (a.verbo === 'listar' && a.sessoes?.length) {
      return a.sessoes.slice(0, 3).map((s) => `${s.nome} ${s.estado}`).join(' · ');
    }
    return '';
  }

  const janela = $derived(
    [passos[0]?.hora, passos[passos.length - 1]?.hora].filter(Boolean).join('→'),
  );
</script>

<div class="ht">
  <div class="ht-cab">
    <svg width="18" height="13" viewBox="0 0 20 14" fill="none" aria-hidden="true">
      <path d="M2 13a8 8 0 0116 0" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" />
      <path d="M6.5 13a3.5 3.5 0 017 0" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" opacity=".55" />
    </svg>
    <span class="ht-titulo">{m.hangar_trilha_titulo({ n: passos.length })}</span>
    <span class="ht-tempo">
      {[janela, total != null ? `${(total / 1000).toFixed(1)} s` : null].filter(Boolean).join(' · ')}
    </span>
  </div>
  <ol class="ht-passos">
    {#each passos as p, i (i)}
      <li class="ht-passo" class:agora={i === passos.length - 1} class:erro={!!p.acao.erro}>
        <span class="ht-linha" aria-hidden="true"></span>
        <span class="ht-txt">
          <span class="ht-verbo">{titulo(p.acao)}</span>
          {#if p.hora}<span class="ht-hora">{p.hora}</span>{/if}
          {#if detalhe(p.acao)}<span class="ht-det">{detalhe(p.acao)}</span>{/if}
        </span>
      </li>
    {/each}
  </ol>
</div>

<style>
  .ht {
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    background: var(--surface-raised);
    overflow: hidden;
    margin: var(--space-1) 0;
  }
  .ht-cab {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--fill-subtle);
    color: var(--accent);
  }
  .ht-titulo { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .ht-tempo { margin-left: auto; font-size: 10.5px; color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .ht-passos { list-style: none; margin: 0; padding: var(--space-2) var(--space-3) var(--space-3); }
  .ht-passo { position: relative; padding: 6px 0 6px 20px; font-size: var(--text-sm); }
  /* Bolinha + o fio que desce até o próximo passo. O fio some no último (não há próximo). */
  .ht-passo::before {
    content: '';
    position: absolute;
    left: 3px;
    top: 12px;
    width: 7px;
    height: 7px;
    border-radius: var(--radius-full);
    background: var(--success);
  }
  .ht-passo.erro::before { background: var(--error); }
  .ht-passo.agora::before { background: var(--accent); box-shadow: 0 0 0 4px var(--accent-dim); }
  .ht-linha {
    position: absolute;
    left: 6px;
    top: 21px;
    bottom: -6px;
    width: 1px;
    background: var(--border-default);
  }
  .ht-passo:last-child .ht-linha { display: none; }
  .ht-txt { display: block; min-width: 0; }
  .ht-verbo { color: var(--text-primary); }
  .ht-passo.agora .ht-verbo { font-weight: 600; }
  .ht-hora { margin-left: 6px; font-size: 10.5px; color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .ht-det {
    display: block;
    margin-top: 2px;
    font-size: 11.5px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>

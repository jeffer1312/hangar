<script lang="ts">
  import * as m from '../paraglide/messages';
  import type { AcaoHangar } from '../lib/hangarCmd';

  // Cartão de um comando do hangar rodado pelo agente. O que ele acrescenta ao card de Bash: diz o
  // que ACONTECEU (em vez da linha de comando), quando e quanto levou, e oferece a ação seguinte
  // óbvia. A saída crua fica num bloco fechado — o cartão pode ler errado, a saída não.
  interface Props {
    acao: AcaoHangar;
    comando: string;
    saida: string;
    /** ms entre a chamada e o resultado. Ausente enquanto roda. */
    duracao?: number | null;
    hora?: string | null;
    /** Abre outra sessão no app (o chat sabe fazer isso; aqui só sai o nome). */
    onAbrirSessao?: (nome: string) => void;
  }
  let { acao, comando, saida, duracao = null, hora = null, onAbrirSessao }: Props = $props();

  const titulo = $derived.by(() => {
    if (acao.erro) {
      return acao.verbo === 'criar'
        ? m.hangar_cmd_falha_criar({ nome: acao.alvo ?? '?' })
        : m.hangar_cmd_falha();
    }
    switch (acao.verbo) {
      case 'criar': return m.hangar_cmd_criou({ nome: acao.alvo ?? '?' });
      // Sem lista lida (saída redirecionada pro /dev/null, por exemplo) o título não inventa "0
      // sessões" — ele só diz o que o comando foi fazer.
      case 'listar': return acao.sessoes?.length
        ? m.hangar_cmd_listou({ n: acao.sessoes.length })
        : m.hangar_cmd_listou_sem_saida();
      case 'recado': return m.hangar_cmd_recado({ nome: acao.alvo ?? '?' });
      case 'parear': return m.hangar_cmd_pareou({ nome: acao.alvo ?? '?' });
      case 'desparear': return m.hangar_cmd_desparaeu();
      case 'grupo': return m.hangar_cmd_grupo({ n: acao.peers?.length ?? 0 });
    }
  });

  // `400 motor invalido` não diz nada pra quem está no celular. O que a gente conhece vira frase;
  // o resto continua cru (dizer errado seria pior que dizer técnico).
  const motivo = $derived.by(() => {
    const e = acao.erro ?? '';
    if (/motor inv[aá]lido/i.test(e)) return m.hangar_cmd_erro_motor();
    if (/sess(ã|a)o.*(existe|duplicad)/i.test(e)) return m.hangar_cmd_erro_nome_ocupado();
    if (/backend inacess/i.test(e)) return m.hangar_cmd_erro_backend();
    return e;
  });

  const tempo = $derived(
    [hora, duracao != null ? `${(duracao / 1000).toFixed(1)} s` : null].filter(Boolean).join(' · '),
  );

  const estadoRotulo: Record<string, () => string> = {
    working: m.hangar_cmd_estado_working,
    awaiting_input: m.hangar_cmd_estado_awaiting,
    idle: m.hangar_cmd_estado_idle,
  };

  let cruAberto = $state(false);
  let copiado = $state(false);
  let falhouCopia = $state(false);
  async function copiar() {
    try {
      await navigator.clipboard.writeText(comando);
      copiado = true;
      setTimeout(() => (copiado = false), 1500);
    } catch {
      // Sem permissão de área de transferência (http em rede local, por exemplo). A falha APARECE
      // no botão e abre a saída crua, que é onde o comando está escrito — dizer nada deixaria a
      // pessoa colando um texto velho sem saber.
      falhouCopia = true;
      cruAberto = true;
      setTimeout(() => (falhouCopia = false), 2500);
    }
  }
</script>

<div class="hc" class:erro={!!acao.erro}>
  <div class="hc-cab">
    <svg class="hc-arco" width="18" height="13" viewBox="0 0 20 14" fill="none" aria-hidden="true">
      <path d="M2 13a8 8 0 0116 0" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" />
      <path d="M6.5 13a3.5 3.5 0 017 0" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" opacity=".55" />
    </svg>
    <span class="hc-titulo">{titulo}</span>
    {#if tempo}<span class="hc-tempo">{tempo}</span>{/if}
  </div>

  <div class="hc-corpo">
    {#if acao.erro}
      <p class="hc-motivo">{motivo}</p>
    {:else if acao.verbo === 'criar'}
      <div class="hc-chips">
        {#if acao.provider}<span class="hc-chip destaque">{acao.provider}</span>{/if}
        {#if acao.motor}<span class="hc-chip destaque">{acao.motor}</span>{/if}
        {#if acao.conta}<span class="hc-chip">{acao.conta}</span>{/if}
        {#if acao.worktree}<span class="hc-chip wt">worktree</span>{/if}
      </div>
      {#if acao.cwd}<div class="hc-cwd">{acao.cwd}</div>{/if}
    {:else if acao.verbo === 'listar' && acao.sessoes}
      <ul class="hc-lista">
        {#each acao.sessoes as s (s.nome)}
          <li class="hc-sessao">
            <span class="hc-bola" data-estado={s.estado} aria-hidden="true"></span>
            <button class="hc-nome" onclick={() => onAbrirSessao?.(s.nome)} disabled={!onAbrirSessao}>{s.nome}</button>
            <span class="hc-estado">{(estadoRotulo[s.estado] ?? (() => s.estado))()}</span>
            <span class="hc-cam">{s.cwd}</span>
          </li>
        {/each}
      </ul>
    {:else if acao.texto}
      <p class="hc-msg">{acao.texto}</p>
      {#if acao.verbo === 'recado'}
        <div class="hc-chips">
          <span class="hc-chip">{acao.enfileirado ? m.hangar_cmd_na_fila() : m.hangar_cmd_entregue()}</span>
        </div>
      {:else if acao.peers?.length}
        <div class="hc-chips">
          {#each acao.peers as p (p)}<span class="hc-chip">{p}</span>{/each}
        </div>
      {/if}
    {/if}

    <div class="hc-acoes">
      {#if acao.alvo && onAbrirSessao && !acao.erro && acao.verbo !== 'listar'}
        <button class="hc-btn primaria" onclick={() => onAbrirSessao(acao.alvo!)}>
          {m.hangar_cmd_abrir({ nome: acao.alvo })}
        </button>
      {/if}
      <button class="hc-btn mudo" class:falhou={falhouCopia} onclick={copiar}>
        {falhouCopia ? m.hangar_cmd_copia_falhou() : copiado ? m.hangar_cmd_copiado() : m.hangar_cmd_copiar()}
      </button>
    </div>

    {#if saida.trim()}
      <details class="hc-cru" bind:open={cruAberto}>
        <summary>{m.hangar_cmd_saida_original({ n: saida.trim().split('\n').length })}</summary>
        <pre>$ {comando}
{saida.trim()}</pre>
      </details>
    {/if}
  </div>
</div>

<style>
  /* Faixa lateral = desfecho. Erro NÃO pinta o cartão inteiro: o conteúdo tem que continuar
     legível, e o motivo já ocupa o lugar do resultado. */
  .hc {
    border: 1px solid var(--border-subtle);
    border-left: 3px solid var(--success);
    border-radius: var(--radius-lg);
    background: var(--surface-raised);
    overflow: hidden;
    margin: var(--space-1) 0;
    animation: bubble-in 180ms ease-out both;
  }
  .hc.erro { border-left-color: var(--error); }
  .hc-cab {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--fill-subtle);
  }
  .hc-arco { flex-shrink: 0; color: var(--accent); }
  .hc.erro .hc-arco { color: var(--error); }
  .hc-titulo {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hc-tempo {
    margin-left: auto;
    flex-shrink: 0;
    font-size: 10.5px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .hc-corpo { padding: var(--space-2) var(--space-3) var(--space-3); display: flex; flex-direction: column; gap: var(--space-2); }
  .hc-chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .hc-chip {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    background: var(--fill-subtle);
    color: var(--text-secondary);
  }
  .hc-chip.destaque { background: var(--accent-dim); color: var(--accent); font-weight: 600; }
  .hc-chip.wt { background: var(--accent-dim); color: var(--accent); font-weight: 700; font-size: 9.5px; letter-spacing: 0.03em; }
  .hc-cwd { font-family: var(--font-mono); font-size: 11.5px; color: var(--text-muted); word-break: break-all; }
  .hc-msg {
    margin: 0;
    font-size: var(--text-sm);
    line-height: 1.5;
    padding: var(--space-2);
    border-radius: var(--radius-md);
    background: var(--fill-subtle);
    border-left: 2px solid var(--accent);
    white-space: pre-wrap;
  }
  .hc-motivo { margin: 0; font-size: var(--text-sm); line-height: 1.5; color: var(--text-primary); }
  .hc-lista { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
  .hc-sessao { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); min-width: 0; }
  .hc-bola { width: 7px; height: 7px; border-radius: var(--radius-full); flex-shrink: 0; background: var(--text-muted); }
  .hc-bola[data-estado='working'] { background: var(--success); }
  .hc-bola[data-estado='awaiting_input'] { background: var(--warning); }
  .hc-nome { padding: 0; background: none; border: none; color: var(--text-primary); font-weight: 600; font-size: var(--text-sm); cursor: pointer; white-space: nowrap; flex-shrink: 0; }
  .hc-nome:disabled { cursor: default; }
  .hc-estado { font-size: 11px; color: var(--text-muted); }
  .hc-cam { margin-left: auto; font-family: var(--font-mono); font-size: 10.5px; color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hc-acoes { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .hc-btn {
    font-size: 11.5px;
    padding: 5px 11px;
    border: none;
    border-radius: var(--radius-full);
    background: var(--fill-subtle);
    color: var(--text-primary);
    cursor: pointer;
  }
  .hc-btn.primaria { background: var(--accent-dim); color: var(--accent); font-weight: 600; }
  .hc-btn.mudo { background: transparent; color: var(--text-muted); padding-left: 0; }
  .hc-btn.falhou { color: var(--error); }
  .hc-cru { border-top: 1px dashed var(--border-subtle); padding-top: var(--space-2); }
  .hc-cru summary { font-size: 11.5px; color: var(--text-muted); cursor: pointer; list-style: none; }
  .hc-cru summary::before { content: '▸ '; }
  .hc-cru[open] summary::before { content: '▾ '; }
  .hc-cru pre {
    margin: var(--space-2) 0 0;
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.55;
    color: var(--text-secondary);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>

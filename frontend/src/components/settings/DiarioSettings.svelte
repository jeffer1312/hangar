<script lang="ts">
  // Tela do diário de uso. Linha PRÓPRIA no menu de Configurações, e não um pedaço de "Sobre",
  // porque o uso dela é sempre o mesmo pedido: "baixa o arquivo e me manda". Enterrada dentro de
  // outra tela, a pessoa teria de procurar — que é exatamente o que este botão existe pra evitar.
  //
  // O que o arquivo contém e o que NÃO contém está em backend/app/diag.py e em lib/diag.ts. O texto
  // aqui repete a parte que importa pra quem vai enviar: fica na máquina, e não guarda conversa.
  import { getDiagResumo, baixarDiag, type ResumoDiag, type LinhaDiag } from '../../lib/api';
  import { fmtBytes, relativeTime } from '../../lib/format';
  import * as m from '../../paraglide/messages';

  let resumo = $state<ResumoDiag | null>(null);
  let erro = $state('');
  let baixando = $state(false);
  let recarregando = $state(false);

  // Uma linha do diário virando texto de UMA linha na tela. O `detalhe` já vem curto do backend
  // (teto de 300) e é o que carrega método+rota; o resto é contexto.
  function contexto(l: LinhaDiag): string {
    const partes = [l.detalhe, l.sessao, l.tela, l.codigo && `#${l.codigo}`,
                    l.ms !== undefined && `${l.ms}ms`];
    return partes.filter(Boolean).join(' · ');
  }

  async function carregar() {
    recarregando = true;
    try {
      resumo = await getDiagResumo();
      erro = '';
    } catch (e) {
      // Backend mais velho que o app (rota ainda não existe) NÃO é erro pra quem lê: some o número
      // e o botão fica desligado. Só falha de verdade aparece.
      erro = (e as { status?: number })?.status === 404 ? '' : (e instanceof Error ? e.message : String(e));
      resumo = null;
    } finally {
      recarregando = false;
    }
  }

  $effect(() => { void carregar(); });

  async function baixar() {
    baixando = true; erro = '';
    try {
      const blob = await baixarDiag();
      // Download do próprio navegador, a partir de um Blob local — nada é enviado a lugar nenhum.
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = 'hangar-uso.jsonl';
      a.click();
      // Revoga num tiquetaque: revogar na mesma volta cancela o download no Safari.
      setTimeout(() => URL.revokeObjectURL(url), 0);
    } catch (e) {
      erro = e instanceof Error ? e.message : String(e);
    } finally {
      baixando = false;
    }
  }
</script>

<div class="diario">
  <p class="desc">{m.config_diag_desc({ dias: String(resumo?.dias_guardados ?? 7) })}</p>

  <ul class="regras">
    <li>{m.config_diag_regra_local()}</li>
    <li>{m.config_diag_regra_sem_conversa()}</li>
    <li>{m.config_diag_regra_rotativo({ dias: String(resumo?.dias_guardados ?? 7) })}</li>
  </ul>

  <button class="btn primario" onclick={baixar} disabled={baixando || !resumo?.dias}>
    {baixando ? m.config_diag_baixando() : m.config_diag_baixar()}
  </button>

  {#if resumo}
    <span class="num">
      {resumo.dias
        ? m.config_diag_tem({ dias: String(resumo.dias), tam: fmtBytes(resumo.bytes) })
        : m.config_diag_vazio()}
    </span>
  {/if}
  {#if erro}<p class="erro" role="alert">{erro}</p>{/if}

  <!-- Prévia das últimas linhas. Existe pra a pessoa CONFERIR que está gravando: sem ela o botão
       de baixar é fé, e quem recebe é que descobre que veio vazio. -->
  <div class="preview-head">
    <strong class="preview-tit">{m.config_diag_ultimas()}</strong>
    <button class="recarregar" onclick={carregar} disabled={recarregando}
            aria-label={m.arq_recarregar()}>{recarregando ? '…' : '↻'}</button>
  </div>

  {#if resumo?.ultimas?.length}
    <ol class="linhas">
      {#each resumo.ultimas as l, i (l.ts + l.evento + i)}
        <li class="linha {l.nivel ?? 'ok'}">
          <span class="quando">{relativeTime(Date.parse(l.ts) / 1000)}</span>
          <span class="evento">{l.evento}</span>
          <span class="ctx">{contexto(l)}</span>
        </li>
      {/each}
    </ol>
  {:else if resumo}
    <p class="vazio">{m.config_diag_vazio()}</p>
  {/if}
</div>

<style>
  .diario { padding: var(--space-2) var(--space-4) var(--space-5); }
  .desc { margin: 0 0 var(--space-3); color: var(--text-secondary); font-size: var(--text-sm); line-height: 1.5; }
  .regras {
    margin: 0 0 var(--space-4);
    padding: var(--space-3) var(--space-3) var(--space-3) var(--space-5);
    /* --surface-inset, não --bg-base: acompanha o slider de transparência quando há papel de
       parede (regra de CSS do CLAUDE.md). */
    background: var(--surface-inset);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    line-height: 1.6;
  }
  .btn {
    width: 100%;
    height: 48px;
    border-radius: var(--radius-md);
    font-size: var(--text-base);
    font-weight: 600;
  }
  .primario { background: var(--accent); color: #fff; }
  .primario:disabled { opacity: 0.5; cursor: default; }
  .num { display: block; margin-top: var(--space-2); text-align: center; font-size: var(--text-xs); color: var(--text-muted); }
  .erro { margin: var(--space-2) 0 0; color: var(--error); font-size: var(--text-sm); }

  .preview-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: var(--space-5) 0 var(--space-2);
  }
  .preview-tit { font-size: var(--text-sm); color: var(--text-secondary); font-weight: 600; }
  .recarregar {
    width: 30px; height: 30px;
    border-radius: var(--radius-sm);
    /* --surface-raised, não --bg-elevated: acompanha o véu do papel de parede. */
    background: var(--surface-raised);
    border: 1px solid var(--border-subtle);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }
  .recarregar:disabled { opacity: 0.5; }

  .linhas {
    list-style: none;
    margin: 0;
    padding: var(--space-2);
    background: var(--surface-inset);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    /* Altura fixa com rolagem própria: a lista é longa por natureza e não pode empurrar o botão
       de baixar pra fora da tela — ele é o motivo desta tela existir. */
    max-height: 340px;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }
  .linha {
    display: grid;
    grid-template-columns: 7ch minmax(9ch, auto) 1fr;
    gap: var(--space-2);
    align-items: baseline;
    padding: 3px var(--space-1);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    border-bottom: 1px solid var(--border-subtle);
  }
  .linha:last-child { border-bottom: 0; }
  .quando { color: var(--text-muted); white-space: nowrap; }
  .evento { color: var(--text-primary); font-weight: 600; }
  /* A linha inteira rola na horizontal quando o contexto é longo — a tela nunca rola junto. */
  .ctx { color: var(--text-secondary); overflow-x: auto; white-space: nowrap; }
  .linha.aviso .evento { color: var(--warning); }
  .linha.erro .evento { color: var(--error); }
  .vazio { margin: 0; padding: var(--space-4); text-align: center; color: var(--text-muted); font-size: var(--text-sm); }
</style>

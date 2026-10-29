<script lang="ts">
  // Tela do diário de uso. Linha PRÓPRIA no menu de Configurações, e não um pedaço de "Sobre",
  // porque o uso dela é sempre o mesmo pedido: "baixa o arquivo e me manda". Enterrada dentro de
  // outra tela, a pessoa teria de procurar — que é exatamente o que este botão existe pra evitar.
  //
  // O que o arquivo contém e o que NÃO contém está em backend/app/diag.py e em lib/diag.ts. O texto
  // aqui repete a parte que importa pra quem vai enviar: fica na máquina, e não guarda conversa.
  import { getDiagResumo, baixarDiag, type ResumoDiag, type LinhaDiag } from '@hangar/core';
  import { fmtBytes, sendDiagReportAll, resendDiagReportMissed, diagReportMachines, diagReportRetryable, diagReportTargets, DIAG_REPORT_TEXT_MAX,
    type DiagReportAllResult, type DiagReportResult, type DiagReportScope } from '@hangar/core';
  import { findServer, getActiveId, listServers, onServersChanged } from '../../lib/auth';
  import { papelDo } from '../../lib/papel.svelte';
  import { copyText } from '../../lib/clipboard';
  import * as m from '../../paraglide/messages';

  // O relatório vai ao mesmo servidor que a prévia e o download acima leem: o ativo.
  const ativo = () => findServer(getActiveId() ?? '') ?? null;
  let servidor = $state(ativo());
  let all = $state(listServers());
  $effect(() => onServersChanged(() => { servidor = ativo(); all = listServers(); }));
  // Convidado não manda relatório da máquina de outra pessoa (o servidor recusa com 403).
  const podeReportar = $derived(!!servidor && !servidor.invite && papelDo(servidor) !== 'guest');
  // O erro costuma estar noutra máquina da pessoa (a VM, não o PC de onde ela envia): o padrão é mandar de todas.
  const machines = $derived(diagReportMachines(all, servidor, (s) => papelDo(s) === 'guest'));

  let texto = $state('');
  let enviando = $state(false);
  let scope = $state<DiagReportScope>('all');
  let picked = $state<Record<string, boolean>>({});
  let resultado = $state<DiagReportAllResult | null>(null);
  let copiado = $state(false);

  const targets = $derived(diagReportTargets(machines, servidor, scope, picked));
  const client = `pwa ${__HANGAR_VERSION__}`;

  async function reportar() {
    if (!targets.length || enviando) return;
    enviando = true; resultado = null; copiado = false;
    resultado = await sendDiagReportAll(targets, { text: texto, client });
    enviando = false;
  }

  // Envio que já tem código e máquinas que não foram: reenvia só a elas, no mesmo código.
  async function resendMissed() {
    if (!resultado?.code || enviando) return;
    enviando = true;
    resultado = await resendDiagReportMissed(resultado, { text: texto, client });
    enviando = false;
  }

  function reason(r: DiagReportResult): string {
    return r.ok ? '' : r.kind === 'unsupported' ? m.config_diag_report_unsupported()
      : r.kind === 'expired' ? m.config_diag_report_expired()
      : r.kind === 'rate_limited' ? m.config_diag_report_rate_limited()
      : r.kind === 'unreachable' ? m.config_diag_report_unreachable()
      : m.config_diag_report_failed({ code: r.code });
  }
  const missed = $derived(resultado?.results.filter((r) => !r.result.ok) ?? []);
  const diaryFailed = $derived(resultado?.results.some((r) => r.result.ok && r.result.diary === 'failed') ?? false);
  const onlyUnsupported = $derived(!!resultado && resultado.code === null && missed.every((r) => !r.result.ok && r.result.kind === 'unsupported'));
  const retryable = $derived(missed.some((r) => diagReportRetryable(r.result)));
  // Quantas máquinas o envio teve, não a escolha de agora: trocar a escolha depois não muda o texto do erro.
  const several = $derived((resultado?.results.length ?? 0) > 1);

  async function copiar(code: string) {
    if (!(await copyText(code))) return;
    copiado = true;
    setTimeout(() => { copiado = false; }, 2000);
  }

  let resumo = $state<ResumoDiag | null>(null);
  let erro = $state('');
  let baixando = $state(false);
  let recarregando = $state(false);

  // Horário ABSOLUTO e de largura fixa, não "há 2 minutos": a coluna relativa varia de largura
  // ("agora" vs "há 14 minutos") e transbordava por cima do nome do evento; e pra cruzar com um
  // relato ("travou umas 10h50") o relógio é o que serve. Dia diferente de hoje ganha a data na
  // frente — a prévia atravessa os sete dias guardados.
  function quando(ts: string): string {
    const d = new Date(ts);
    if (Number.isNaN(d.getTime())) return '--:--:--';
    const hora = d.toLocaleTimeString(undefined, { hour12: false });
    const hoje = new Date();
    const mesmoDia = d.toDateString() === hoje.toDateString();
    return mesmoDia ? hora : `${d.getDate()}/${d.getMonth() + 1} ${hora}`;
  }

  // O contexto de UMA linha. O `detalhe` já vem curto do backend (teto de 300) e carrega
  // método+rota; o resto é o que localiza o evento. O código HTTP só aparece quando NÃO deu certo:
  // um "#200" em toda linha de ação é ruído que empurra pra fora o que importa.
  function contexto(l: LinhaDiag): string {
    const codigo = l.nivel && l.nivel !== 'ok' ? l.codigo : undefined;
    const plataforma = l.so && `${l.so} · ${l.navegador ?? ''} · ${l.vista ?? ''} · ${l.tela_px ?? ''}`;
    return [l.detalhe, plataforma, l.sessao, l.tela, codigo && `#${codigo}`,
            l.ms !== undefined && `${l.ms}ms`].filter(Boolean).join(' · ');
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
  <!-- Só os marcadores: o parágrafo que ficava aqui dizia as MESMAS três coisas logo acima deles,
       e ler a mesma frase duas vezes seguidas faz a tela parecer defeituosa. -->
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

  {#if servidor && podeReportar}
    <section class="reportar" aria-labelledby="diag-report-tit">
      <strong id="diag-report-tit" class="preview-tit">{m.config_diag_report_title()}</strong>
      <p class="rep-servidor">{machines.length > 1 ? m.config_diag_report_server_many() : m.config_diag_report_server({ server: servidor.label })}</p>
      {#if resultado?.code}
        {@const code = resultado.code}
        <p class="rep-msg" role="status">{m.config_diag_report_sent()}</p>
        <div class="rep-codigo">
          <code>{code}</code>
          <button class="recarregar" onclick={() => copiar(code)}>
            {copiado ? m.config_diag_report_copied() : m.config_diag_report_copy()}
          </button>
        </div>
        {#if diaryFailed}<p class="rep-msg aviso">{m.config_diag_report_diary_failed()}</p>{/if}
        {#if retryable}
          <button class="btn primario" onclick={resendMissed} disabled={enviando}>
            {enviando ? m.config_diag_report_sending() : m.config_diag_report_retry()}
          </button>
        {/if}
      {:else}
        {#if machines.length > 1}
          <fieldset class="rep-escopo" disabled={enviando}>
            <legend class="rep-label">{m.config_diag_report_scope_label()}</legend>
            <label><input type="radio" bind:group={scope} value="all" /> {m.config_diag_report_scope_all({ count: machines.length })}</label>
            <label><input type="radio" bind:group={scope} value="this" /> {m.config_diag_report_scope_this({ server: servidor.label })}</label>
            <label><input type="radio" bind:group={scope} value="pick" /> {m.config_diag_report_scope_pick()}</label>
            {#if scope === 'pick'}
              <div class="rep-maquinas">
                {#each machines as s (s.id)}
                  <label><input type="checkbox" checked={picked[s.id] ?? s.id === servidor.id}
                                onchange={(e) => { picked[s.id] = e.currentTarget.checked; }} /> {s.label}</label>
                {/each}
              </div>
            {/if}
          </fieldset>
        {/if}
        <label class="rep-label" for="diag-report-texto">{m.config_diag_report_text_label()}</label>
        <textarea id="diag-report-texto" bind:value={texto} maxlength={DIAG_REPORT_TEXT_MAX} rows="3"
                  disabled={enviando} placeholder={m.config_diag_report_text_placeholder()}></textarea>
        <p class="rep-aviso">{m.config_diag_report_notice()}</p>
        <button class="btn primario" onclick={reportar} disabled={enviando || !targets.length || onlyUnsupported}>
          {enviando ? m.config_diag_report_sending()
            : resultado && !onlyUnsupported ? m.config_diag_report_retry()
            : m.config_diag_report_send()}
        </button>
      {/if}
      {#if missed.length}
        <div class="erro" role="alert">
          {#if several}<p>{m.config_diag_report_machines_failed()}</p>{/if}
          <ul class="rep-falhas">
            {#each missed as f (f.server.id)}
              <li>{several ? m.config_diag_report_machine_failed({ server: f.server.label, reason: reason(f.result) }) : reason(f.result)}</li>
            {/each}
          </ul>
        </div>
      {/if}
    </section>
  {/if}

  <!-- Prévia das últimas linhas. Existe pra a pessoa CONFERIR que está gravando: sem ela o botão
       de baixar é fé, e quem recebe é que descobre que veio vazio. -->
  <div class="preview-head">
    <strong class="preview-tit">{m.config_diag_ultimas()}</strong>
    <!-- Rótulo ao lado do ↻: no toque não há hover, e o aria-label sozinho não chega a quem vê. -->
    <button class="recarregar" onclick={carregar} disabled={recarregando}
            aria-label={m.arq_recarregar()}><span aria-hidden="true">{recarregando ? '…' : '↻'}</span>
      <span class="recarregar-txt">{m.arq_recarregar()}</span></button>
  </div>

  {#if resumo?.ultimas?.length}
    <ol class="linhas">
      {#each resumo.ultimas as l, i (l.ts + l.evento + i)}
        <li class="linha {l.nivel ?? 'ok'}">
          <span class="quando">{quando(l.ts)}</span>
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

  .reportar { display: flex; flex-direction: column; gap: var(--space-2); margin-top: var(--space-5); }
  .rep-servidor, .rep-aviso, .rep-msg { margin: 0; font-size: var(--text-sm); color: var(--text-secondary); }
  .rep-aviso { font-size: var(--text-xs); color: var(--text-muted); }
  .rep-msg.aviso { color: var(--warning); }
  .rep-label { font-size: var(--text-sm); color: var(--text-secondary); }
  .rep-escopo { display: flex; flex-direction: column; gap: var(--space-1); margin: 0; padding: 0; border: 0; font-size: var(--text-sm); }
  .rep-escopo label { display: flex; align-items: center; gap: var(--space-2); min-height: 32px; }
  .rep-maquinas { display: flex; flex-direction: column; padding-left: var(--space-5); }
  .rep-falhas { margin: var(--space-1) 0 0; padding-left: var(--space-4); }
  .erro p { margin: 0; }
  textarea {
    width: 100%;
    box-sizing: border-box;
    padding: var(--space-2) var(--space-3);
    background: var(--surface-inset);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font: inherit;
    font-size: var(--text-base);
    resize: vertical;
  }
  .rep-codigo {
    display: flex; align-items: center; justify-content: space-between; gap: var(--space-3);
    padding: var(--space-3);
    background: var(--surface-inset);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .rep-codigo code {
    font-family: var(--font-mono);
    font-size: 1.6rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: var(--text-primary);
    user-select: all;
  }
  .rep-codigo .recarregar { height: 44px; }

  .preview-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: var(--space-5) 0 var(--space-2);
  }
  .preview-tit { font-size: var(--text-sm); color: var(--text-secondary); font-weight: 600; }
  .recarregar {
    /* Largura pelo rótulo, altura de antes: era um quadrado de 30px com só o ↻ dentro. */
    height: 30px;
    display: inline-flex; align-items: center; gap: 5px;
    padding: 0 var(--space-2);
    border-radius: var(--radius-sm);
    /* --surface-raised, não --bg-elevated: acompanha o véu do papel de parede. */
    background: var(--surface-raised);
    border: 1px solid var(--border-subtle);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }
  .recarregar:disabled { opacity: 0.5; }
  .recarregar-txt { font-size: var(--text-xs); white-space: nowrap; }

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
  /* Duas colunas fixas (relógio, evento) e o contexto ocupando o resto. `max-content` nas duas
     primeiras: com `ch` a coluna do relógio ficava mais estreita que o texto e ele transbordava
     POR CIMA do nome do evento (visto na tela em 25/08/2026). `min-width: 0` na terceira é o que
     permite ela encolher — sem isso o grid estoura a largura do painel. */
  .linha {
    display: grid;
    grid-template-columns: max-content max-content 1fr;
    gap: var(--space-2);
    align-items: baseline;
    padding: 4px var(--space-1);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    border-bottom: 1px solid var(--border-subtle);
  }
  .linha:last-child { border-bottom: 0; }
  .quando { color: var(--text-muted); white-space: nowrap; }
  .evento { color: var(--text-primary); font-weight: 600; white-space: nowrap; }
  /* QUEBRA, não rola: num painel estreito, rolagem horizontal por linha esconde o fim da mensagem
     de erro — que é justamente o que a pessoa precisa ler antes de mandar o arquivo. */
  .ctx {
    color: var(--text-secondary);
    min-width: 0;
    overflow-wrap: anywhere;
    word-break: break-word;
  }
  .linha.aviso .evento { color: var(--warning); }
  .linha.erro .evento { color: var(--error); }
  .vazio { margin: 0; padding: var(--space-4); text-align: center; color: var(--text-muted); font-size: var(--text-sm); }
</style>

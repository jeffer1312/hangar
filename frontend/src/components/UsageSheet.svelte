<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { desktop } from '../lib/desktop.svelte';
  import { money2, fmtDur } from '../lib/fmt';
  import { moeda } from '../lib/moeda.svelte';
  import { cachePrazo, type UltimoCache } from '../lib/cachePrazo';
  import type { ContaChip } from '../lib/conta';
  import * as m from '../paraglide/messages';
  import { intlLocale } from '../lib/locale';
  import { abbrevNum } from '@hangar/core';
  import type { StatusFields, StatsEvent } from '@hangar/core';

  interface Props {
    open: boolean;
    status: StatusFields | null;
    onClose: () => void;
    // A faixa de estatística do composer: no celular ela mora só aqui.
    stats?: StatsEvent | null;
    lastCache?: UltimoCache | null;
    title?: string;
    conta?: ContaChip | null;
    // Banner de limite lido do pane: o chip dele abre esta folha, então ela tem que dizer quando volta.
    limited?: boolean;
    limitReset?: string | null;
  }
  let { open, status, onClose, stats = null, lastCache = null, title, conta = null, limited = false, limitReset = null }: Props = $props();

  const known = (n: unknown): n is number => typeof n === 'number' && isFinite(n);
  const clamp = (n: number) => Math.min(100, Math.max(0, n));
  // Mesmos limiares do anel e do chip da cota (70 / 90).
  const tone = (n: number) => (n >= 90 ? 'hot' : n >= 70 ? 'warn' : 'ok');

  const janelas = $derived.by(() => {
    const s = status;
    const out: { label: string; pct: number; reset?: string }[] = [];
    if (!s) return out;
    if (known(s.fiveHourPct)) out.push({ label: m.uso_janela_5h(), pct: s.fiveHourPct, reset: s.fiveHourReset });
    if (known(s.weeklyPct)) out.push({ label: m.uso_janela_7d(), pct: s.weeklyPct, reset: s.weeklyReset });
    if (known(s.monthlyPct)) out.push({ label: m.uso_janela_30d(), pct: s.monthlyPct, reset: s.monthlyReset });
    return out;
  });

  // O prazo anda com o relógio, mas só enquanto a folha está aberta.
  let agora = $state(Date.now());
  $effect(() => {
    if (!open) return;
    agora = Date.now();
    const id = setInterval(() => (agora = Date.now()), 20_000);
    return () => clearInterval(id);
  });
  const cache = $derived(cachePrazo(lastCache, agora));

  const ctxPct = $derived(status?.ctxPct);
  const ctxTokens = $derived.by(() => {
    const used = status?.ctxUsed;
    if (!known(used)) return '';
    const total = status?.ctxTotal;
    return used.toLocaleString(intlLocale()) + (total ? ' / ' + total.toLocaleString(intlLocale()) : '');
  });

  const linhas = $derived.by(() => {
    const s = status;
    const out: { label: string; value: string }[] = [];
    if (!s) return out;
    if (known(s.costUsd)) out.push({ label: m.uso_custo(), value: money2(s.costUsd, moeda.cur, moeda.rate) });
    if (s.sessionTime) out.push({ label: m.uso_tempo_sessao(), value: s.sessionTime });
    if (s.model) out.push({ label: m.composer_modelo(), value: s.model + (s.effort ? ` · ${s.effort}` : '') });
    return out;
  });

  const numeros = $derived.by(() => {
    const s = stats;
    if (!s) return [] as { value: string; label: string; largo?: boolean }[];
    const out: { value: string; label: string; largo?: boolean }[] = [];
    // Turno zero com chamadas é sessão em que o turno não é contado (sem terminal), não "0 turnos".
    if (s.turns > 0 || !s.steps) out.push({ value: String(s.turns), label: m.uso_num_turnos() });
    out.push({ value: String(s.steps), label: m.uso_num_chamadas() });
    if (s.llm_ms) out.push({ value: fmtDur(s.llm_ms), label: m.uso_num_llm() });
    if (s.tool_ms) out.push({ value: fmtDur(s.tool_ms), label: m.uso_num_tools() });
    // O rótulo do nativo não diz a unidade; ela vai no valor, como lá.
    const rate = (v: number, exact?: boolean) =>
      (exact ? m.native_stats_rate_exact : m.native_stats_rate)({ n: String(Math.round(v)) });
    if (s.tok_s) out.push({ value: rate(s.tok_s), label: m.native_ctx_card_rate() });
    if (s.tok_s_now) out.push({ value: rate(s.tok_s_now, s.tok_s_exact), label: m.native_ctx_card_rate_now() });
    if (s.tok_s_recent) out.push({ value: rate(s.tok_s_recent, s.tok_s_exact), label: m.native_ctx_card_rate_recent() });
    if (s.ttft_ms) out.push({ value: `~${fmtDur(s.ttft_ms)}`, label: m.uso_num_ttft() });
    if (s.cache_pct != null) out.push({ value: `${s.cache_pct}%`, label: m.uso_num_cache() });
    out.push({ value: `${abbrevNum(s.in_tok)} / ${abbrevNum(s.out_tok)}`, label: m.uso_num_io(), largo: true });
    return out;
  });

  const temConversa = $derived(known(ctxPct) || !!lastCache || !!status?.repo || linhas.length > 0);
  const vazio = $derived(!limited && !janelas.length && !temConversa && !numeros.length && !status?.raw);
</script>

<BottomSheet {open} {onClose} ariaLabel={m.uso_aria()} centered={desktop.atual}>
  <div class="usage">
    <div class="usage-head">
      <h2 class="usage-title">{title ?? m.uso_titulo()}</h2>
      {#if conta}
        <span class="usage-conta" title={m.sessao_conta({ n: conta.nome })}>
          <span class="conta-dot" style="background: {conta.cor};" aria-hidden="true"></span>{conta.label}
        </span>
      {/if}
    </div>

    {#if vazio}
      <p class="usage-vazio">{m.uso_vazio()}</p>
    {/if}

    {#if janelas.length || limited}
      <section class="usage-sec">
        <h3 class="usage-sec-title">{m.uso_secao_cota()}</h3>
        {#if limited}
          <div class="usage-row">
            <span class="usage-label">{m.sessao_limite()}</span>
            <span class="usage-value usage-limite">{limitReset ? m.rate_volta({ quando: limitReset }) : m.rate_limitado()}</span>
          </div>
        {/if}
        {#each janelas as j}
          <div class="medidor tone-{tone(j.pct)}">
            <div class="medidor-head"><span>{j.label}</span><span class="medidor-pct">{Math.round(clamp(j.pct))}%</span></div>
            <div class="medidor-bar"><span style:width={`${clamp(j.pct)}%`}></span></div>
            {#if j.reset}<span class="medidor-sub">{m.uso_reset({ quando: j.reset })}</span>{/if}
          </div>
        {/each}
      </section>
    {/if}

    {#if temConversa}
      <section class="usage-sec">
        <h3 class="usage-sec-title">{m.uso_secao_conversa()}</h3>
        {#if known(ctxPct)}
          <div class="medidor tone-{tone(ctxPct)}">
            <div class="medidor-head"><span>{m.ctx_contexto()}</span><span class="medidor-pct">{Math.round(clamp(ctxPct))}%</span></div>
            <div class="medidor-bar"><span style:width={`${clamp(ctxPct)}%`}></span></div>
            {#if ctxTokens}<span class="medidor-sub">{ctxTokens}</span>{/if}
          </div>
        {/if}
        {#if lastCache}
          <div class="usage-row">
            <span class="usage-label">{m.uso_cache_prompt()}</span>
            <span class="usage-value cache" class:acabando={cache.acabando} class:frio={!cache.ativo}>
              <span class="cache-dot" aria-hidden="true"></span>{cache.label}
            </span>
          </div>
        {/if}
        {#if status?.repo}
          <div class="usage-row">
            <span class="usage-label">{m.uso_linha_projeto()}</span>
            <span class="usage-value">
              {status.repo}{#if status.branch}<span class="sep" aria-hidden="true">·</span><span class="mono">{status.branch}</span>{/if}
              {#if status.dirty}<span class="usage-dirty">{m.composer_alteracoes_nao_commitadas()}</span>{/if}
            </span>
          </div>
        {/if}
        {#each linhas as r}
          <div class="usage-row">
            <span class="usage-label">{r.label}</span>
            <span class="usage-value">{r.value}</span>
          </div>
        {/each}
      </section>
    {/if}

    {#if numeros.length}
      <section class="usage-sec">
        <h3 class="usage-sec-title">{m.uso_secao_numeros()}</h3>
        <div class="numeros">
          {#each numeros as n}
            <div class="numero" class:largo={n.largo}><span class="numero-v">{n.value}</span><span class="numero-l">{n.label}</span></div>
          {/each}
        </div>
      </section>
    {/if}

    {#if status?.raw}
      <div class="usage-raw">
        <span class="usage-label">{m.uso_statusline()}</span>
        <code class="usage-raw-line">{status.raw}</code>
      </div>
    {/if}
  </div>
</BottomSheet>

<style>
  .usage { display: flex; flex-direction: column; gap: var(--space-5); padding: var(--space-2) 0; font-variant-numeric: tabular-nums; }
  .usage-head { display: flex; flex-direction: column; gap: 2px; }
  .usage-title { font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); margin: 0; }
  .usage-conta { font-size: var(--text-xs); color: var(--text-muted); }
  .conta-dot { display: inline-block; width: 6px; height: 6px; margin-right: 6px; border-radius: 50%; vertical-align: 1px; }
  .usage-vazio { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }

  .usage-sec { display: flex; flex-direction: column; gap: var(--space-3); }
  .usage-sec-title { margin: 0; font-size: var(--text-xs); font-weight: 600; color: var(--text-muted); }

  .medidor { display: flex; flex-direction: column; gap: 6px; }
  .medidor-head { display: flex; justify-content: space-between; gap: var(--space-3); font-size: var(--text-sm); color: var(--text-primary); }
  .medidor-pct { font-weight: 600; }
  .medidor-bar { height: 6px; overflow: hidden; border-radius: var(--radius-full); background: var(--fill-subtle); }
  .medidor-bar > span { display: block; height: 100%; min-width: 2px; border-radius: inherit; background: var(--accent); transition: width 600ms var(--ease-out); }
  .medidor-sub { font-size: var(--text-xs); color: var(--text-muted); }
  .medidor.tone-warn .medidor-bar > span { background: var(--warning); }
  .medidor.tone-warn .medidor-pct { color: var(--warning); }
  .medidor.tone-hot .medidor-bar > span { background: var(--error); }
  .medidor.tone-hot .medidor-pct { color: var(--error); }

  .usage-row { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-4); }
  .usage-label { font-size: var(--text-sm); color: var(--text-secondary); flex-shrink: 0; }
  .usage-value { font-size: var(--text-sm); color: var(--text-primary); text-align: right; min-width: 0; overflow-wrap: anywhere; }
  .mono { font-family: var(--font-mono); }
  .usage-dirty { display: block; font-size: var(--text-xs); color: var(--warning); }
  .usage-limite { color: var(--warning); font-weight: 600; }
  .cache { display: inline-flex; align-items: center; gap: 6px; font-weight: 600; }
  .cache-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--success); }
  .cache.acabando { color: var(--warning); }
  .cache.acabando .cache-dot { background: var(--warning); }
  .cache.frio { color: var(--text-muted); font-weight: 400; }
  .cache.frio .cache-dot { background: var(--text-muted); opacity: 0.5; }

  .numeros { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: var(--space-2); }
  .numero { display: flex; flex-direction: column; gap: 2px; min-width: 0; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md); background: var(--fill-subtle); }
  .numero-v { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .numero-l { font-size: var(--text-xs); color: var(--text-muted); }
  .numero.largo { grid-column: span 2; }
  .sep { margin: 0 0.35em; color: var(--text-muted); }

  .usage-raw { display: flex; flex-direction: column; gap: var(--space-1); padding-top: var(--space-3); border-top: 1px solid var(--border-subtle); }
  .usage-raw-line { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); word-break: break-all; white-space: pre-wrap; }
</style>

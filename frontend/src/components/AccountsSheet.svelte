<script lang="ts">
  // Contas no celular, no desenho do cartão de contas do desktop (accounts/usage.rs): uma linha por
  // conta com as barras Sessão (5h) e Semana (7d); tocar leva a conversa para ela. Abre pela conta no
  // topo do chat e pelo anel da conta na faixa do campo.
  import { onMount } from 'svelte';
  import * as m from '../paraglide/messages';
  import BottomSheet from './BottomSheet.svelte';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import { useSessionServer } from '../lib/sessionServer';
  import { quotaFeed } from '../lib/quotaFeed.svelte';
  import { formatarIntervalo } from '../lib/contaEstado';
  import {
    faixaDeCota, faltaPara, diaDoReset, janelaLonga, motivoParado, motivoSessaoViva, exhaustedWindow,
    type ContaCota, type JanelaExibida,
  } from '../lib/cota';
  import { listAccountTargets, type AccountTarget, type CliProxyAccount } from '@hangar/core';

  interface Props {
    open: boolean;
    onClose: () => void;
    sessionName: string;
    serverKey: string;
    /** Conta da sessão aberta, como o /api/cotas a identifica. */
    activeAccount: string | null;
    /** Ausente: a sessão não troca de conta Claude (não é Claude na conta Anthropic, ou tem motor). */
    onSwitch?: (target: AccountTarget) => void;
    /** Troca só com a sessão parada. */
    blocked: boolean;
    /** Sessão trabalhando: o motivo aparece e "Interromper" para ela. */
    onInterrupt?: () => void;
    /** Sessão com motor GPT: as contas ChatGPT do proxy, e a troca entre elas. */
    engineAccounts?: CliProxyAccount[] | null;
    activeEngineAccount?: string | null;
    onSwitchEngineAccount?: (account: CliProxyAccount) => void;
    /** Só escolher uma conta Claude (retomar noutra conta): sem troca da sessão nem Interromper. */
    pickOnly?: boolean;
    onPick?: (configDir: string) => void;
    onOpenSettings?: () => void;
  }
  let {
    open, onClose, sessionName, serverKey, activeAccount, onSwitch, blocked, onInterrupt,
    engineAccounts = null, activeEngineAccount = null, onSwitchEngineAccount, pickOnly = false, onPick, onOpenSettings,
  }: Props = $props();
  const sessionServer = useSessionServer();

  onMount(() => {
    quotaFeed.retain();
    return () => quotaFeed.release();
  });
  $effect(() => { void serverKey; quotaFeed.setServidor(serverKey); });

  let targets = $state<AccountTarget[] | null>(null);
  let targetsError = $state('');
  let seq = 0;
  $effect(() => {
    if (!open || !onSwitch || pickOnly) return;
    const mine = ++seq;
    targets = null;
    targetsError = '';
    listAccountTargets(sessionName, sessionServer())
      .then((list) => { if (mine === seq) targets = list; })
      .catch((e) => { if (mine === seq) targetsError = e instanceof Error ? e.message : String(e); });
  });

  const accounts = $derived(
    faixaDeCota(quotaFeed.contas.map((c) => (c.ts == null ? c : { ...c, idade_s: Math.max(0, quotaFeed.agora - c.ts) }))),
  );
  // Em uso primeiro, como o cartão do nativo; Claude antes do resto.
  const claude = $derived((accounts ?? []).filter((c) => c.provedor === 'claude')
    .sort((a, b) => Number(b.id === activeAccount) - Number(a.id === activeAccount)));
  const others = $derived((accounts ?? []).filter((c) => c.provedor !== 'claude' && !c.id.startsWith('codex:')));
  const codex = $derived((accounts ?? []).filter((c) => c.id.startsWith('codex:')));
  const freshest = $derived.by(() => {
    const ages = (accounts ?? []).map((c) => c.idade_s).filter((i): i is number => i != null);
    return ages.length ? Math.min(...ages) : null;
  });

  const PROVIDER_NAMES: Record<string, string> = { claude: 'Claude', codex: 'Codex', kimi: 'Kimi', opencode: 'OpenCode' };
  const providerName = (p: string) => PROVIDER_NAMES[p] ?? p.charAt(0).toUpperCase() + p.slice(1);

  // Até duas barras: a de 5h vira "Sessão", a de 7d "Semana" (meter do nativo).
  function meters(c: ContaCota): JanelaExibida[] {
    const gerais = c.janelas.filter((j) => !j.porModelo);
    const pick = [gerais.find((j) => j.rotulo === '5h'), gerais.find((j) => j.rotulo === '7d')].filter((j): j is JanelaExibida => !!j);
    return pick.length ? pick : gerais.slice(0, 2);
  }
  function meterLabel(j: JanelaExibida): string {
    return j.rotulo === '5h' ? m.accounts_window_session() : j.rotulo === '7d' ? m.accounts_window_week() : j.rotulo;
  }
  function resetOf(j: JanelaExibida): string {
    const r = janelaLonga(j.resetTs, quotaFeed.agora) ? diaDoReset(j.resetTs, quotaFeed.agora) : faltaPara(j.resetTs, quotaFeed.agora);
    return r ? `↺ ${r}` : '';
  }
  const pathOf = (c: ContaCota) => c.id.slice('claude:'.length);
  const targetOf = (c: ContaCota) => targets?.find((t) => `claude:${t.path}` === c.id) ?? null;
  function exhausted(c: ContaCota): boolean { return !!exhaustedWindow(c) || !!targetOf(c)?.full; }
  function claudeTappable(c: ContaCota): boolean {
    if (exhausted(c) || c.id === activeAccount) return false;
    if (pickOnly) return !!onPick;
    return !!onSwitch && !blocked && !!targetOf(c);
  }
  function tapClaude(c: ContaCota) {
    if (!claudeTappable(c)) return;
    onClose();
    if (pickOnly) { onPick?.(pathOf(c)); return; }
    const t = targetOf(c);
    if (t) onSwitch?.(t);
  }
  const quotaOfCredential = (cred: string) => (accounts ?? []).find((c) => c.id === cred) ?? null;
</script>

{#snippet bars(c: ContaCota)}
  {#if c.janelas.length === 0}
    <span class="vazio">
      {c.estado === 'expirada' || c.estado === 'sem_credencial'
        ? (motivoSessaoViva(c.motivo) ? m.cota_sessao_viva() : motivoParado(c.motivo) ? m.cota_conta_parada() : m.cota_precisa_entrar())
        : '—'}
    </span>
  {:else}
    <span class="meters">
      {#each meters(c) as j (j.rotulo)}
        <span class="meter">
          <span class="ml">{meterLabel(j)}</span>
          <span class="trilho"><i class="barra barra--{j.nivel}" style="width:{Math.min(100, j.pct)}%"></i></span>
          <b class="pct pct--{j.nivel}">{Math.round(j.pct)}%</b>
          <span class="reset">{resetOf(j)}</span>
        </span>
      {/each}
    </span>
  {/if}
{/snippet}

{#snippet row(c: ContaCota, tappable: boolean, onTap: () => void, inUse: boolean)}
  {@const dim = exhausted(c)}
  {#if tappable}
    <button type="button" class="conta tap" onclick={onTap}>
      <span class="who"><span class="nome">{c.label}</span>{#if inUse}<span class="tag">{m.cota_conta_ativa()}</span>{/if}</span>
      {@render bars(c)}
    </button>
  {:else}
    <div class="conta" class:dim aria-disabled={dim ? 'true' : undefined}>
      <span class="who">
        <span class="nome">{c.label}</span>
        {#if inUse}<span class="tag">{m.cota_conta_ativa()}</span>{/if}
        {#if dim}<span class="tag tag--cheio">{m.accounts_exhausted()}</span>{/if}
        {#if c.velha && c.idade_s != null}<span class="idade">{m.cota_idade({ n: formatarIntervalo(c.idade_s) })}</span>{/if}
      </span>
      {@render bars(c)}
    </div>
  {/if}
{/snippet}

<BottomSheet {open} {onClose} ariaLabel={m.accounts_sheet_title()}>
  <div class="acc">
    <div class="acc-head">
      <h2 class="acc-title">{m.accounts_sheet_title()}</h2>
      <span class="acc-att">
        {#if freshest != null}{m.cota_idade({ n: formatarIntervalo(freshest) })} · {/if}
        <button type="button" class="link" onclick={() => quotaFeed.atualizar()}>{m.cota_atualizar()}</button>
      </span>
    </div>

    {#if !pickOnly}
      {#if blocked && (onSwitch || onSwitchEngineAccount)}
        <div class="busy" role="status">
          <p>{m.native_usage_card_move_busy()}</p>
          {#if onInterrupt}<button type="button" class="busy-btn" onclick={() => { onInterrupt?.(); }}>{m.composer_interromper()}</button>{/if}
        </div>
      {:else if onSwitch}
        <p class="note">{m.accounts_tap_to_move({ sessao: sessionName })}</p>
      {/if}
      {#if targetsError}<p class="note error" role="alert">{m.conta_erro_listar({ erro: targetsError })}</p>{/if}
    {/if}

    {#if accounts === null}
      <p class="note">{m.comum_carregando()}</p>
    {:else if accounts.length === 0}
      <p class="note">{m.accounts_none()}</p>
    {:else}
      {#if engineAccounts?.length && !pickOnly}
        <div class="prov"><ProviderGlyph provider="codex" size={15} /><span>{m.native_create_chatgpt_account()}</span></div>
        {#each engineAccounts as a (a.account)}
          {@const q = quotaOfCredential(a.credential_id)}
          {@const inUse = a.account === activeEngineAccount}
          {#if !inUse && !blocked && onSwitchEngineAccount && !(q && exhausted(q))}
            <button type="button" class="conta tap" onclick={() => { onClose(); onSwitchEngineAccount?.(a); }}>
              <span class="who"><span class="nome">{a.label || a.email || a.account}</span></span>
              {#if q}{@render bars(q)}{/if}
            </button>
          {:else}
            <div class="conta" class:dim={!!q && exhausted(q)}>
              <span class="who"><span class="nome">{a.label || a.email || a.account}</span>{#if inUse}<span class="tag">{m.cota_conta_ativa()}</span>{/if}</span>
              {#if q}{@render bars(q)}{/if}
            </div>
          {/if}
        {/each}
      {/if}

      {#if claude.length}
        <div class="prov"><ProviderGlyph provider="claude" size={15} /><span>Claude</span></div>
        {#each claude as c (c.id)}
          {@render row(c, claudeTappable(c), () => tapClaude(c), c.id === activeAccount)}
        {/each}
      {/if}

      {#if codex.length && !engineAccounts?.length && !pickOnly}
        <div class="prov"><ProviderGlyph provider="codex" size={15} /><span>Codex</span></div>
        {#if onSwitch}<p class="note">{m.accounts_codex_note()}</p>{/if}
        {#each codex as c (c.id)}{@render row(c, false, () => {}, c.id === activeAccount)}{/each}
      {/if}

      {#each others as c (c.id)}
        <div class="prov"><ProviderGlyph provider={c.provedor ?? 'claude'} size={15} /><span>{providerName(c.provedor ?? '')}</span></div>
        {@render row(c, false, () => {}, c.id === activeAccount)}
      {/each}
    {/if}

    {#if onOpenSettings}
      <button type="button" class="settings" onclick={() => { onClose(); onOpenSettings?.(); }}>{m.accounts_configure()}</button>
    {/if}
  </div>
</BottomSheet>

<style>
  .acc { display: flex; flex-direction: column; gap: var(--space-1); padding: var(--space-2) var(--space-4) var(--space-5); }
  .acc-head { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-2); }
  .acc-title { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .acc-att { font-size: var(--text-xs); color: var(--text-muted); }
  .link { min-height: 44px; min-width: 0; padding: 0 var(--space-1); color: var(--accent); font-size: var(--text-xs); }
  .note { margin: var(--space-1) var(--space-2); font-size: var(--text-sm); color: var(--text-muted); }
  .note.error { color: var(--error); }
  .busy {
    display: flex; align-items: center; gap: var(--space-3); margin: var(--space-1) 0; padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md); background: var(--surface-raised); font-size: var(--text-sm); color: var(--text-secondary);
  }
  .busy p { margin: 0; flex: 1; }
  .busy-btn { min-height: 44px; padding: 0 var(--space-3); border-radius: var(--radius-md); background: var(--accent-dim); color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .prov { display: flex; align-items: center; gap: var(--space-2); margin-top: var(--space-3); font-size: var(--text-xs); font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--text-muted); }
  .conta {
    display: flex; flex-direction: column; gap: 6px; width: 100%; min-height: 56px; padding: var(--space-2);
    border-radius: var(--radius-md); background: transparent; text-align: left; color: inherit;
  }
  .conta.tap { cursor: pointer; align-items: stretch; justify-content: flex-start; text-align: left; }
  .conta.tap:active { background: var(--bg-hover); }
  .conta.dim { opacity: 0.55; }
  .who { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .nome { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .tag { font-size: var(--text-xs); color: var(--accent); }
  .tag--cheio { color: var(--error); }
  .idade, .vazio { font-size: var(--text-xs); color: var(--text-muted); }
  .meters { display: flex; flex-direction: column; gap: 4px; width: 100%; }
  .meter { display: grid; grid-template-columns: 4.6em 1fr 3em 7.6em; align-items: center; gap: var(--space-2); font-size: var(--text-xs); color: var(--text-secondary); }
  .ml { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .trilho { height: 4px; border-radius: 2px; background: var(--bg-hover); overflow: hidden; }
  .barra { display: block; height: 100%; border-radius: 2px; background: var(--accent); }
  .barra--alerta { background: var(--warning); }
  .barra--cheio { background: var(--error); }
  .pct { text-align: right; font-variant-numeric: tabular-nums; color: var(--text-primary); }
  .pct--alerta { color: var(--warning); }
  .pct--cheio { color: var(--error); }
  .reset { color: var(--text-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .settings { min-height: 44px; margin-top: var(--space-3); border-radius: var(--radius-md); border: 1px solid var(--border-subtle); background: transparent; color: var(--text-primary); font-size: var(--text-sm); }
</style>

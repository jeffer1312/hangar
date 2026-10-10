<script lang="ts">
  // Contas no celular, aberta pela conta no topo do chat: trocar a conta desta conversa e a cota
  // de todas as contas, o que no desktop fica na pílula de cota.
  import { onMount } from 'svelte';
  import * as m from '../paraglide/messages';
  import BottomSheet from './BottomSheet.svelte';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import { useSessionServer } from '../lib/sessionServer';
  import { quotaFeed } from '../lib/quotaFeed.svelte';
  import { formatarIntervalo } from '../lib/contaEstado';
  import {
    faixaDeCota, faltaPara, diaDoReset, janelaLonga, motivoParado, motivoSessaoViva,
    type ContaCota, type JanelaExibida,
  } from '../lib/cota';
  import { listAccountTargets, type AccountTarget } from '@hangar/core';

  interface Props {
    open: boolean;
    onClose: () => void;
    sessionName: string;
    serverKey: string;
    /** Conta da sessão aberta, como o /api/cotas a identifica. */
    activeAccount: string | null;
    /** Ausente: a sessão não troca de conta (não é Claude na conta Anthropic, ou tem motor). */
    onSwitch?: (target: AccountTarget) => void;
    /** Troca só com a sessão parada. */
    blocked: boolean;
  }
  let { open, onClose, sessionName, serverKey, activeAccount, onSwitch, blocked }: Props = $props();
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
    if (!open || !onSwitch) return;
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
  const groups = $derived.by(() => {
    const byProvider = new Map<string, ContaCota[]>();
    for (const c of accounts ?? []) {
      const k = c.provedor ?? 'claude';
      if (!byProvider.has(k)) byProvider.set(k, []);
      byProvider.get(k)!.push(c);
    }
    return [...byProvider.entries()];
  });
  const freshest = $derived.by(() => {
    const ages = (accounts ?? []).map((c) => c.idade_s).filter((i): i is number => i != null);
    return ages.length ? Math.min(...ages) : null;
  });

  const PROVIDER_NAMES: Record<string, string> = { claude: 'Claude', codex: 'Codex', kimi: 'Kimi', opencode: 'OpenCode' };
  const providerName = (p: string) => PROVIDER_NAMES[p] ?? p.charAt(0).toUpperCase() + p.slice(1);

  function resetOf(j: JanelaExibida): string {
    const r = janelaLonga(j.resetTs, quotaFeed.agora) ? diaDoReset(j.resetTs, quotaFeed.agora) : faltaPara(j.resetTs, quotaFeed.agora);
    return r ? `↺ ${r}` : '';
  }

  function pick(t: AccountTarget) {
    onClose();
    onSwitch?.(t);
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={m.accounts_sheet_title()}>
  <div class="acc">
    <h2 class="acc-title">{m.accounts_sheet_title()}</h2>

    <h3 class="acc-sec">{m.accounts_switch_title()}</h3>
    {#if !onSwitch}
      <p class="note">{m.conta_mesma_so_claude()}</p>
    {:else if targetsError}
      <p class="note error" role="alert">{m.conta_erro_listar({ erro: targetsError })}</p>
    {:else if targets === null}
      <p class="note">{m.comum_carregando()}</p>
    {:else if targets.length === 0}
      <p class="note">{m.conta_nenhuma_outra()}</p>
    {:else}
      {#if blocked}<p class="note">{m.modo_so_ociosa()}</p>{/if}
      {#each targets as t (t.path)}
        <button class="item" type="button" onclick={() => pick(t)} disabled={blocked || t.full}>
          <span class="txt">
            <span class="label">{t.label}</span>
            <span class="sub">{t.pct === null ? m.conta_cota_sem_leitura()
              : t.full ? m.conta_cota_cheia({ pct: String(Math.round(t.pct)) })
              : t.low ? m.conta_cota_acabando({ pct: String(Math.round(t.pct)) })
              : m.conta_cota_uso({ pct: String(Math.round(t.pct)) })}</span>
          </span>
          <span class="chev" aria-hidden="true">›</span>
        </button>
      {/each}
    {/if}

    <div class="acc-sec-row">
      <h3 class="acc-sec">{m.cota_uso_contas()}</h3>
      <span class="acc-att">
        {#if freshest != null}{m.cota_idade({ n: formatarIntervalo(freshest) })} · {/if}
        <button type="button" class="link" onclick={() => quotaFeed.atualizar()}>{m.cota_atualizar()}</button>
      </span>
    </div>
    {#if accounts === null}
      <p class="note">{m.comum_carregando()}</p>
    {:else if accounts.length === 0}
      <p class="note">{m.accounts_none()}</p>
    {:else}
      {#each groups as [provider, list] (provider)}
        <div class="prov"><ProviderGlyph {provider} size={15} /><span>{providerName(provider)}</span></div>
        {#each list as c (c.id)}
          <div class="conta" class:velha={c.velha}>
            <div class="conta-nome">
              {c.label}
              {#if c.id === activeAccount}<span class="tag">{m.cota_conta_ativa()}</span>{/if}
              {#if c.velha && c.idade_s != null}<span class="idade">{m.cota_idade({ n: formatarIntervalo(c.idade_s) })}</span>{/if}
            </div>
            {#if c.janelas.length === 0}
              <div class="vazio">
                {c.estado === 'expirada' || c.estado === 'sem_credencial'
                  ? (motivoSessaoViva(c.motivo) ? m.cota_sessao_viva()
                    : motivoParado(c.motivo) ? m.cota_conta_parada() : m.cota_precisa_entrar())
                  : '—'}
              </div>
            {:else}
              {#each c.janelas as j (j.rotulo)}
                <div class="jan">
                  <span class="jq">{j.rotulo}</span>
                  <span class="trilho"><i class="barra barra--{j.nivel}" style="width:{Math.min(100, j.pct)}%"></i></span>
                  <b class="pct pct--{j.nivel}">{Math.round(j.pct)}%</b>
                  <span class="reset">{resetOf(j)}</span>
                </div>
              {/each}
            {/if}
          </div>
        {/each}
      {/each}
    {/if}
  </div>
</BottomSheet>

<style>
  .acc { display: flex; flex-direction: column; gap: var(--space-1); padding: var(--space-2) var(--space-4) var(--space-5); }
  .acc-title { margin: 0 0 var(--space-2); font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .acc-sec { margin: var(--space-3) 0 var(--space-1); font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary); }
  .acc-sec-row { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-2); }
  .acc-att { font-size: var(--text-xs); color: var(--text-muted); }
  .link { min-height: 44px; min-width: 0; padding: 0 var(--space-1); color: var(--accent); font-size: var(--text-xs); }
  .note { margin: var(--space-1) var(--space-2); font-size: var(--text-sm); color: var(--text-muted); }
  .note.error { color: var(--error); }
  .item {
    display: flex; align-items: center; gap: var(--space-3); width: 100%; min-height: 56px;
    padding: var(--space-2); background: transparent; border-radius: var(--radius-md); text-align: left;
  }
  .item:active { background: var(--bg-hover); }
  .item:disabled { opacity: 0.45; }
  .txt { display: flex; flex-direction: column; gap: 1px; flex: 1; min-width: 0; }
  .label { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .sub { font-size: var(--text-xs); color: var(--text-muted); }
  .chev { color: var(--text-muted); font-size: var(--text-lg); }
  .prov { display: flex; align-items: center; gap: var(--space-2); margin-top: var(--space-2); font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .conta { padding: var(--space-2); border-radius: var(--radius-md); }
  .conta.velha { opacity: 0.7; }
  .conta-nome { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); font-size: var(--text-sm); color: var(--text-primary); }
  .tag { font-size: var(--text-xs); color: var(--accent); }
  .idade, .vazio { font-size: var(--text-xs); color: var(--text-muted); }
  .jan { display: grid; grid-template-columns: 3.2em 1fr 3em 8.8em; align-items: center; gap: var(--space-2); margin-top: var(--space-1); font-size: var(--text-xs); color: var(--text-secondary); }
  .trilho { height: 6px; border-radius: 3px; background: var(--bg-hover); overflow: hidden; }
  .barra { display: block; height: 100%; border-radius: 3px; background: var(--text-muted); }
  .barra--alerta { background: var(--warning); }
  .barra--cheio { background: var(--error); }
  .pct { text-align: right; font-variant-numeric: tabular-nums; color: var(--text-primary); }
  .pct--alerta { color: var(--warning); }
  .pct--cheio { color: var(--error); }
  .reset { color: var(--text-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .jq { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>

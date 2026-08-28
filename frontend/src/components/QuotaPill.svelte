<script lang="ts">
  // Pílula de cota na barra do topo (referência: o medidor do super.engineering — "o ícone mostra
  // o provider; clique abre o detalhe; expandir mostra todas"). A pílula mostra a conta da SESSÃO
  // ATIVA (pedido do usuário); sem sessão aberta ou sem leitura dela, cai no pior-geral (modo
  // smart, `piorJanela` em lib/cota) — um relance responde "tenho com o que trabalhar?". O clique
  // abre o popover agrupado por provider, e a faixa do rodapé (QuotaStrip) vira o modo expandido
  // ("Mostrar na barra" no rodapé da caixa).
  //
  // Desenho dos números: círculo na pílula (gauge = saúde), barra no detalhe (progresso) — a
  // regra do Vercel Geist pra quota/ratio. Cores: as mesmas da faixa (neutro até 80%, âmbar até
  // 90%, vermelho acima — `nivelDePct` em lib/cota). O número é % USADO, como sempre foi na faixa.
  import { onMount } from 'svelte';
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import { quotaFeed } from '../lib/quotaFeed.svelte';
  import { quotaBarra } from '../lib/quotaBarra.svelte';
  import { formatarIntervalo } from '../lib/contaEstado';
  import {
    faixaDeCota, piorJanela, faltaPara, diaDoReset, janelaLonga,
    motivoParado, motivoSessaoViva, type ContaCota, type JanelaExibida,
  } from '../lib/cota';

  interface Props {
    /** ID do SERVIDOR ativo — o feed relê na máquina certa (ver QuotaStrip). Só o servidor, nunca
     *  `<servidor>::<sessão>`: cota é da credencial da máquina, não da sessão aberta, e com o nome
     *  da sessão dentro toda troca de aba jogava as contas fora (pílula em branco) e refazia o GET
     *  — medido em 28/08/2026, 559ms disputando com o histórico em cada abertura de sessão. */
    serverKey: string;
    /** Conta da sessão ABERTA (id do /api/cotas). Quando existe e tem leitura, a pílula mostra o
     *  uso DELA (pedido do usuário); sem ela, cai no pior-geral (smart). */
    contaAtiva: string | null;
    onIrParaContas: () => void;
  }
  let { serverKey, contaAtiva, onIrParaContas }: Props = $props();

  let aberto = $state(false);
  let pillEl = $state<HTMLButtonElement | null>(null);

  onMount(() => {
    quotaFeed.retain();
    return () => quotaFeed.release();
  });
  $effect(() => { void serverKey; quotaFeed.setServidor(serverKey); });

  const contasComIdade = $derived(
    quotaFeed.contas.map((c) => (c.ts == null ? c : { ...c, idade_s: Math.max(0, quotaFeed.agora - c.ts) })),
  );
  const linha = $derived(faixaDeCota(contasComIdade));

  // O que a pílula mostra: a janela mais cheia DA CONTA DA SESSÃO ATIVA quando ela existe e tem
  // leitura; senão o pior-geral (smart). `piorJanela` com uma conta só serve pros dois lados.
  const contaDaSessao = $derived(linha?.find((c) => c.id === contaAtiva) ?? null);
  const mostrada = $derived.by(() => {
    if (contaDaSessao?.janelas.length) {
      const p = piorJanela([contaDaSessao]);
      if (p) return { ...p, smart: false };
    }
    const p = piorJanela(linha);
    return p ? { ...p, smart: true } : null;
  });

  // Idade da leitura mais FRESCA — o "atualizado agora" do rodapé fala do feed, não de uma conta.
  const fresca = $derived.by(() => {
    const idades = contasComIdade.map((c) => c.idade_s).filter((i): i is number => i != null);
    return idades.length ? Math.min(...idades) : null;
  });

  // Grupos por provider, na ordem de primeira aparição (Claude costuma vir primeiro no /api/cotas).
  const grupos = $derived.by(() => {
    const mapa = new Map<string, ContaCota[]>();
    for (const c of linha ?? []) {
      const k = c.provedor ?? 'claude';
      if (!mapa.has(k)) mapa.set(k, []);
      mapa.get(k)!.push(c);
    }
    return [...mapa.entries()];
  });

  function resetDe(j: JanelaExibida): string {
    const r = janelaLonga(j.resetTs, quotaFeed.agora)
      ? diaDoReset(j.resetTs, quotaFeed.agora)
      : faltaPara(j.resetTs, quotaFeed.agora);
    return r ? `↺ ${r}` : '';
  }

  // Rótulo do grupo de provider no popover. NÃO é o providerName de lib/format (domínio de
  // SESSÃO, que cai em 'Claude' pra qualquer desconhecido): aqui 'opencode' precisa virar
  // OpenCode, e o que não for conhecido mostra a chave capitalizada em vez de mentir.
  const NOMES_PROV: Record<string, string> = { claude: 'Claude', kimi: 'Kimi', opencode: 'OpenCode' };
  function nomeProvedor(p: string): string {
    return NOMES_PROV[p] ?? p.charAt(0).toUpperCase() + p.slice(1);
  }

  // O anel da pílula: raio 7, circunferência ≈ 43.98. O arco desenha o % USADO da janela mostrada.
  const CIRC = 43.98;
  const arco = $derived(mostrada ? CIRC * (1 - Math.min(100, mostrada.janela.pct) / 100) : CIRC);

  function irParaContas() {
    aberto = false;
    onIrParaContas();
  }
</script>

{#if linha}
  <button
    bind:this={pillEl}
    type="button"
    class="quota-pill"
    class:aberta={aberto}
    onclick={() => (aberto = !aberto)}
    aria-expanded={aberto}
    aria-label={mostrada
      ? (mostrada.smart
        ? m.cota_pill_aria({ p: Math.round(mostrada.janela.pct), j: mostrada.janela.rotulo, c: mostrada.conta.label })
        : m.cota_pill_aria_ativa({ p: Math.round(mostrada.janela.pct), j: mostrada.janela.rotulo, c: mostrada.conta.label }))
      : m.cota_uso_contas()}
    title={m.cota_uso_contas()}
  >
    <!-- O ícone é o provider da conta mostrada (o "icon shows the active provider" deles). -->
    {#if mostrada}<ProviderGlyph provider={mostrada.conta.provedor} size={14} />{/if}
    <span class="qp-gauge" aria-hidden="true">
      <svg width="16" height="16" viewBox="0 0 16 16">
        <circle cx="8" cy="8" r="7" fill="none" stroke="var(--bg-hover)" stroke-width="2.4" />
        {#if mostrada}
          <circle
            class="qp-arco qp-arco--{mostrada.janela.nivel}"
            cx="8" cy="8" r="7" fill="none" stroke-width="2.4" stroke-linecap="round"
            stroke-dasharray={CIRC} stroke-dashoffset={arco}
          />
        {/if}
      </svg>
    </span>
    {#if mostrada}
      <b class="qp-num qp-num--{mostrada.janela.nivel}">{Math.round(mostrada.janela.pct)}%</b>
      <span class="qp-quem">{mostrada.janela.rotulo} · {mostrada.conta.label}</span>
    {:else}
      <!-- Contas conhecidas sem nenhuma leitura: traço, nunca zero inventado (mesma regra da faixa). -->
      <span class="qp-quem">—</span>
    {/if}
  </button>

  <Popover open={aberto} anchor={pillEl} onClose={() => (aberto = false)} width={384} maxH={640} ariaLabel={m.cota_uso_contas()}>
    <div class="qp-pop">
      <div class="qp-head">
        <strong>{m.cota_uso_contas()}</strong>
        <span class="qp-att">
          {#if fresca != null}{m.cota_idade({ n: formatarIntervalo(fresca) })} · {/if}
          <button type="button" class="qp-link" data-foco onclick={() => quotaFeed.atualizar()}>{m.cota_atualizar()}</button>
        </span>
      </div>

      <div class="qp-corpo">
        {#each grupos as [prov, contas] (prov)}
          <div class="qp-prov">
            <ProviderGlyph provider={prov} size={15} />
            <span class="qp-prov-nome">{nomeProvedor(prov)}</span>
          </div>
          {#each contas as c (c.id)}
            <div class="qp-conta" class:velha={c.velha} class:ativa-sessao={c.id === contaAtiva}>
              <div class="qp-conta-nome">
                {#if c.ativa}<span class="qp-base" aria-hidden="true"></span>{/if}
                {c.label}
                {#if c.id === contaAtiva}
                  <!-- A conta da sessão aberta — a mesma que a pílula está mostrando. -->
                  <span class="qp-tag-ativa">{m.cota_conta_ativa()}</span>
                {/if}
                {#if c.velha && c.idade_s != null}
                  <span class="qp-idade">{m.cota_idade({ n: formatarIntervalo(c.idade_s) })}</span>
                {/if}
              </div>
              {#if c.janelas.length === 0}
                <div class="qp-vazio">
                  {c.estado === 'expirada' || c.estado === 'sem_credencial'
                    ? (motivoSessaoViva(c.motivo) ? m.cota_sessao_viva()
                      : motivoParado(c.motivo) ? m.cota_conta_parada() : m.cota_precisa_entrar())
                    : '—'}
                </div>
              {:else}
                {#each c.janelas as j (j.rotulo)}
                  <div class="qp-jan">
                    <div class="qp-jan-linha">
                      <span class="qp-jq">{j.rotulo}</span>
                      <span class="qp-det">
                        <b class="qp-pct qp-pct--{j.nivel}">{Math.round(j.pct)}%</b>
                        {#if resetDe(j)}<span class="qp-reset">{resetDe(j)}</span>{/if}
                      </span>
                    </div>
                    <div class="qp-trilho"><i class="qp-barra qp-barra--{j.nivel}" style="width:{Math.min(100, j.pct)}%"></i></div>
                  </div>
                {/each}
              {/if}
            </div>
          {/each}
        {/each}
      </div>

      <div class="qp-rodape">
        <button type="button" class="qp-link" onclick={irParaContas}>{m.contas_titulo()}</button>
        <button type="button" class="qp-link qp-toggle" onclick={() => quotaBarra.alternar()}>
          {quotaBarra.aberta ? m.cota_esconder_barra() : m.cota_mostrar_barra()}
        </button>
      </div>
    </div>
  </Popover>
{/if}

<style>
  /* A pílula mora entre o ⋯ e o ⚙ da SessionTabs e fala a MESMA língua dos vizinhos: transparente
     em repouso (como .tab-action), vidro só no hover — uma bolha com borda própria ali parecia
     elemento colado de fora (queixa do usuário no primeiro uso real). */
  .quota-pill {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 10px 0 8px;
    border-radius: var(--radius-full);
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-secondary);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    cursor: pointer;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out), border-color 160ms var(--ease-out);
  }
  .quota-pill:hover { background: var(--bg-hover); color: var(--text-primary); }
  .quota-pill:active { transform: scale(0.97); }
  /* Aberta: o vocabulário da aba ativa (accent-dim + filete accent), não uma borda nova. */
  .quota-pill.aberta { background: var(--accent-dim); color: var(--text-primary); }
  .qp-gauge { display: grid; place-items: center; }
  .qp-gauge svg { transform: rotate(-90deg); }
  .qp-arco { transition: stroke-dashoffset 400ms var(--ease-out); }
  /* Cor segue o nível da faixa: neutro até 80% — a pílula quieta é a boa notícia. */
  .qp-arco--normal { stroke: var(--text-muted); }
  .qp-arco--alerta { stroke: var(--warning); }
  .qp-arco--cheio { stroke: var(--error); }
  .qp-num { font-weight: 700; }
  .qp-num--alerta { color: var(--warning); }
  .qp-num--cheio { color: var(--error); }
  /* Rótulo da pior conta pode ser longo ("DeepSeek · opencode direto…"): trunca, não empurra
     a engrenagem pra fora da barra. */
  .qp-quem { color: var(--text-muted); max-width: 18ch; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* Entrada origin-aware (canto da pílula), 180ms — cartilha de polish; some com reduced-motion. */
  .qp-pop {
    display: flex; flex-direction: column; min-height: 0;
    transform-origin: top right;
    animation: qp-pop 180ms var(--ease-out) both;
  }
  @keyframes qp-pop { from { opacity: 0; transform: scale(0.97) translateY(-4px); } }
  @media (prefers-reduced-motion: reduce) { .qp-pop { animation: none; } }

  .qp-head {
    display: flex; align-items: baseline; justify-content: space-between;
    padding: 12px 14px 8px;
  }
  .qp-head strong { font-size: var(--text-sm); }
  .qp-att { color: var(--text-muted); font-size: var(--text-2xs); }

  .qp-corpo { overflow-y: auto; min-height: 0; padding: 0 14px; }

  .qp-prov {
    display: flex; align-items: center; gap: 7px;
    margin: 10px 0 2px;
    color: var(--text-secondary);
  }
  .qp-prov-nome { font-size: 10.5px; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; }

  .qp-conta { padding: 7px 0 8px; }
  .qp-conta + .qp-conta { border-top: 1px solid var(--border-subtle); }
  /* A conta da sessão aberta ganha o filete à esquerda (o mesmo sinal do awaiting na sidebar) —
     sem caixa, que as contas aqui se separam por filete, não por borda. */
  .qp-conta.ativa-sessao { box-shadow: inset 2px 0 0 var(--accent); padding-left: 8px; }
  .qp-tag-ativa {
    font-size: 9.5px; font-weight: 700; letter-spacing: 0.04em; text-transform: uppercase;
    color: var(--accent); background: var(--accent-dim);
    padding: 1px 6px; border-radius: var(--radius-full);
  }
  .qp-conta-nome { font-weight: 650; font-size: var(--text-xs); margin-bottom: 5px; display: flex; align-items: center; gap: 6px; }
  /* A conta-base do app (a que sessão nova nasce usando): o mesmo ponto da faixa do rodapé. */
  .qp-base { width: 5px; height: 5px; border-radius: 50%; background: var(--accent); }
  .qp-idade { color: var(--text-muted); font-weight: 400; }

  .qp-jan { margin-top: 6px; }
  .qp-jan-linha { display: flex; align-items: baseline; justify-content: space-between; margin-bottom: 4px; }
  .qp-jq { color: var(--text-muted); font-size: var(--text-2xs); }
  .qp-det { font-size: var(--text-2xs); color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .qp-pct { font-size: var(--text-xs); font-weight: 700; }
  .qp-pct--alerta { color: var(--warning); }
  .qp-pct--cheio { color: var(--error); }
  .qp-reset { margin-left: 5px; }

  .qp-trilho { height: 6px; border-radius: var(--radius-full); background: var(--bg-hover); overflow: hidden; }
  .qp-barra { display: block; height: 100%; border-radius: var(--radius-full); transition: width 400ms var(--ease-out); }
  .qp-barra--normal { background: var(--text-muted); }
  .qp-barra--alerta { background: var(--warning); }
  .qp-barra--cheio { background: var(--error); }

  .qp-vazio { color: var(--text-muted); font-size: var(--text-2xs); }
  /* Leitura velha parece velha sem apagar: cor cai pro neutro e a idade fica ao lado (da faixa). */
  .qp-conta.velha .qp-pct { color: var(--text-muted); }

  .qp-rodape {
    display: flex; justify-content: space-between; align-items: center;
    margin-top: 8px; padding: 8px 14px 10px;
    border-top: 1px solid var(--border-subtle);
  }
  .qp-link {
    background: none; border: none; padding: 0;
    color: var(--accent); font-size: var(--text-xs); font-weight: 600; cursor: pointer;
  }
  .qp-link:hover { text-decoration: underline; }
  /* O Popover foca o [data-foco] ao abrir: sem estilo próprio o anel default saía um bloco branco
     (visto no print real). Mesmo anel accent do resto do app. */
  .qp-link:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: 3px; }
</style>

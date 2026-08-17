<script lang="ts">
  // Faixa de cota do rodapé do desktop (Task 9). Só existe com duas ou mais contas de que se
  // consiga ler o limite; uma linha por conta, janelas 5h/7d, cor só acima de 80% (sempre junto
  // do número), leitura velha esmaecida com a idade ao lado. Leva à aba Contas por callback — o
  // DesktopShell desvia para a rota (?config=contas), nunca importando o componente da aba
  // (contrato de posse do lote).
  //
  // Fonte única do estado: /api/conta-estado via lib/contaEstado (Task 4) — aqui só se lê o
  // shape dela, nunca o sidecar de statusline por conta própria.
  import { onMount } from 'svelte';
  import * as m from '../paraglide/messages';
  import { listarEstadosDeConta, formatarIntervalo, type ContaEstado } from '../lib/contaEstado';
  import { faixaDeCota } from '../lib/cota';

  interface Props {
    // Muda quando a sessão/servidor alvo muda no shell — re-busca o estado (o endpoint é do
    // servidor ATIVO; sem re-busca a faixa mostraria as contas do servidor antigo para sempre).
    serverKey: string;
    // Leva à aba Contas. Quem constrói é o DesktopShell: () => abrirConfig('contas', getActiveId()).
    onIrParaContas: () => void;
  }
  let { serverKey, onIrParaContas }: Props = $props();

  let contas = $state<ContaEstado[]>([]);
  // Relógio local para a idade andar sem bater na rede: o backend manda o `ts` (timestamp UNIX
  // da escrita do sidecar); a idade exibida é `agora - ts`, reavaliada a cada minuto. Sem isto
  // uma conta lida há 2 min mostraria "lido agora" para sempre (a régua "dado velho parece
  // velho" morre: o dado envelhece, o texto não).
  let agora = $state(Date.now() / 1000);

  const contasComIdade = $derived(
    contas.map((c) => {
      const ts = c.limite.ts;
      if (ts == null) return c;
      return { ...c, limite: { ...c.limite, idade_s: Math.max(0, agora - ts) } };
    }),
  );
  const linha = $derived(faixaDeCota(contasComIdade));

  // Geração descarta resposta em voo de servidor anterior (mesmo papel do `vivo` do shell): o
  // effect reexecuta a cada troca de serverKey; sem o `g` a resposta lenta do servidor antigo
  // sobrescrevia a do novo. `contas = []` em falha: sem dado a faixa some (mesma régua de zero
  // contas legíveis) — não desenha por cima nem mostra erro cru numa tira de 26px.
  let geracao = 0;
  async function carregar() {
    const g = ++geracao;
    try {
      const lista = await listarEstadosDeConta();
      if (g !== geracao) return;
      contas = lista;
    } catch {
      // Falha de rede não apaga leitura boa: o dado que já está na tela envelhece sozinho
      // (o `agora` sobe a cada minuto e a conta vira `velha` com a idade ao lado).
      // Zerar aqui fazia a faixa inteira desaparecer num 500 de um segundo.
    }
  }

  $effect(() => {
    void serverKey;
    void carregar();
  });

  onMount(() => {
    // A faixa é permanente no rodapé: o percentual e a idade precisam andar com o uso. Sem o
    // refetch, uma sessão que acabou de rodar na conta não aparece na faixa até o app remontar.
    const t = setInterval(() => {
      agora = Date.now() / 1000;
      void carregar();
    }, 60_000);
    return () => clearInterval(t);
  });
</script>

{#if linha}
  <div class="quota-faixa">
    <div class="quota-trilho">
      {#each linha as c, i (c.label)}
        {#if i > 0}<span class="quota-sep" aria-hidden="true"></span>{/if}
        <span class="quota-conta" class:velha={c.velha}>
          <span class="quota-nome">{c.label}</span>
          {#if c.cincoH}
            <span class="quota-par">
              <span class="quota-rot">{c.cincoH.rotulo}</span>
              <span class="quota-barra" aria-hidden="true"><i class={c.cincoH.nivel} style="width:{c.cincoH.pct}%"></i></span>
              <span class="quota-num {c.cincoH.nivel}">{c.cincoH.pct}%</span>
            </span>
          {/if}
          {#if c.seteD}
            <span class="quota-par">
              <span class="quota-rot">{c.seteD.rotulo}</span>
              <span class="quota-barra" aria-hidden="true"><i class={c.seteD.nivel} style="width:{c.seteD.pct}%"></i></span>
              <span class="quota-num {c.seteD.nivel}">{c.seteD.pct}%</span>
            </span>
          {/if}
          {#if c.velha && c.idade_s != null}
            <span class="quota-idade">{m.cota_idade({ n: formatarIntervalo(c.idade_s) })}</span>
          {/if}
        </span>
      {/each}
    </div>
    <span class="quota-fim">
      <button type="button" class="quota-link" onclick={onIrParaContas}>{m.contas_titulo()}</button>
    </span>
  </div>
{/if}

<style>
  /* Fiel ao mock (faixa-cota.html): irmã de .shell-linha, flex-shrink:0, altura de uma linha,
     26px + borda. Superfície --glass-panel, o mesmo material da sidebar — acompanha o slider de
     Transparência sozinho, não é retângulo chapado sobre o papel de parede. O trilho da barra
     usa --surface-raised (== --bg-elevated, mas dentro da régua de transparência do app). */
  .quota-faixa {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--space-4);
    height: 26px;
    padding: 0 var(--space-3);
    border-top: 1px solid var(--border-subtle);
    background: var(--glass-panel);
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
  }
  /* Trilho rolável, mesmo desenho da .tabs-strip do SessionTabs: em janela estreita as contas
     saem de vista por rolagem, em vez de encolherem até o nome desaparecer. */
  .quota-trilho {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--space-4);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .quota-trilho::-webkit-scrollbar { display: none; }
  .quota-conta { display: flex; align-items: center; gap: var(--space-2); flex-shrink: 0; }
  .quota-nome {
    color: var(--text-secondary);
    max-width: 12ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .quota-par { display: flex; align-items: center; gap: 5px; }
  .quota-rot { color: var(--text-muted); }
  .quota-barra {
    width: 46px;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-raised);
    overflow: hidden;
  }
  .quota-barra i {
    display: block;
    height: 100%;
    border-radius: 2px;
    background: var(--text-muted);
  }
  .quota-barra i.alerta { background: var(--warning); }
  .quota-barra i.cheio { background: var(--error); }
  .quota-num { font-variant-numeric: tabular-nums; min-width: 3ch; text-align: right; }
  .quota-num.alerta { color: var(--warning); }
  .quota-num.cheio { color: var(--error); }
  /* Dado velho parece velho SEM apagar o texto: quem esmaece é a barrinha (decorativa,
     aria-hidden) e a cor de alerta cai para o tom neutro — a pista textual é a idade ao lado,
     que fica em contraste cheio. opacity no texto não serve: nem 0,90 chega aos 4,5:1 neste fundo. */
  .quota-conta.velha .quota-barra { opacity: 0.45; }
  .quota-conta.velha .quota-num { color: var(--text-muted); }
  .quota-idade { color: var(--text-muted); }
  .quota-sep { width: 1px; height: 12px; background: var(--border-subtle); flex-shrink: 0; }
  .quota-fim { flex-shrink: 0; margin-left: var(--space-3); display: flex; align-items: center; gap: var(--space-2); }
  .quota-link {
    min-height: 20px;
    min-width: 0;
    color: var(--text-muted);
    border: none;
    border-bottom: 1px dotted var(--border-strong);
    background: transparent;
    padding: 0;
    font: inherit;
    cursor: pointer;
  }
</style>
<script lang="ts">
  import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
  import Select from '../Select.svelte';
  import EscopoChip from './EscopoChip.svelte';
  import * as m from '../../paraglide/messages';

  interface Campo {
    chave: string;
    rotulo: string;
    ajuda: string;
    tipo: 'texto' | 'segredo' | 'numero' | 'liga' | 'escolha';
    sufixo?: string;
    opcoes?: { value: string; label: string }[];
  }

  interface Props {
    campo: Campo;
    store: ConfigServidorStore;
    /** Onde este campo grava. Padrão "servidor": quem usa esta linha é o store de servidor.
     *  `null` = vale só neste aparelho, e aí a etiqueta não existe (a regra está dita uma vez na
     *  linha fixa do topo do modal). */
    escopo?: 'servidor' | 'env' | null;
    /** Veredito + motivo da linha vaga ("Recomendado: ligado" + o "por quê?" que expande). Os dois
     *  juntos, ou nenhum: veredito sem motivo é o mesmo texto vago que isto veio resolver. */
    veredito?: string;
    motivo?: string;
  }
  let { campo: c, store, escopo = 'servidor', veredito, motivo }: Props = $props();
  const estado = $derived(store.campos[c.chave]);
</script>

<div class="linha" class:liga={c.tipo === 'liga'}>
  <div class="txt">
    <label class="rot" for={`cfg-${c.chave}`}>
      {c.rotulo}
      {#if escopo}<EscopoChip {escopo} />{/if}
      {#if estado?.origem === 'app'}<span class="tag">{m.config_server_editado()}</span>{/if}
    </label>
    <span class="ajuda">{c.ajuda}</span>
    {#if veredito && motivo}
      <!-- Elemento nativo de expandir/recolher: teclado e estado saem de graça, sem $state nem
           aria próprio. -->
      <details class="porque">
        <summary><span class="vered">{veredito}</span> <span class="pq">{m.config_motores_por_que()}</span></summary>
        <p class="motivo">{motivo}</p>
      </details>
    {/if}
  </div>

  {#if c.tipo === 'liga'}
    <input
      id={`cfg-${c.chave}`}
      class="switch"
      type="checkbox"
      checked={store.valorAtual(c.chave) === true}
      onchange={(e) => store.setRascunho(c.chave, e.currentTarget.checked)}
    />
  {:else if c.tipo === 'escolha'}
    <Select id={`cfg-${c.chave}`} class="campo-select" ariaLabel={c.rotulo}
      value={String(store.valorAtual(c.chave) ?? '')}
      opcoes={c.opcoes ?? []}
      onchange={(v) => store.setRascunho(c.chave, v)} />
  {:else if c.tipo === 'numero'}
    <span class="campo-num">
      <input
        id={`cfg-${c.chave}`}
        type="number"
        inputmode="numeric"
        min="0"
        value={store.valorAtual(c.chave)}
        oninput={(e) => store.setRascunho(c.chave, e.currentTarget.value)}
      />
      {#if c.sufixo}<span class="sufixo">{c.sufixo}</span>{/if}
    </span>
  {:else if c.tipo === 'segredo'}
    <!-- O segredo ENTRA mas não sai. O campo fica VAZIO: pré-preencher com a máscara faz
         qualquer toque no input mandar o texto mascarado de volta e sobrescrever a chave
         real. A máscara aparece ao lado, como informação, não como valor editável. -->
    {#if estado?.definido}
      <span class="mascara" title={m.config_server_chave_nao_volta()}>
        {estado.valor} <span class="mascara-nota">{m.config_server_configurada()}</span>
      </span>
    {/if}
    <input
      id={`cfg-${c.chave}`}
      class="campo-txt"
      type="text"
      autocomplete="off"
      autocapitalize="off"
      spellcheck={false}
      placeholder={estado?.definido ? m.config_motores_colar_nova() : m.config_motores_colar()}
      value={store.rascunhoDe(c.chave)}
      oninput={(e) => store.setRascunho(c.chave, e.currentTarget.value)}
    />
  {:else}
    <input
      id={`cfg-${c.chave}`}
      class="campo-txt"
      type="text"
      autocomplete="off"
      autocapitalize="off"
      spellcheck={false}
      value={store.valorAtual(c.chave)}
      oninput={(e) => store.setRascunho(c.chave, e.currentTarget.value)}
    />
  {/if}
</div>

<style>
  .linha {
    /* Container da etiqueta de escopo: é a LARGURA DESTA LINHA que decide se ela cabe ao lado do
       rótulo, não a da janela (no dock do desktop a janela é larga e a linha tem ~530px). */
    container-type: inline-size;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border-subtle);
  }
  /* Liga/desliga fica na MESMA linha do rótulo: o controle é pequeno e o texto manda. */
  .linha.liga { flex-direction: row; align-items: center; justify-content: space-between; gap: var(--space-4); }

  .txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  /* `wrap`: apertado, a etiqueta de escopo desce inteira pra linha de baixo — sem isso ela rouba a
     largura do rótulo e o texto quebra em uma palavra por linha. */
  .rot {
    display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2);
    font-size: var(--text-base); font-weight: 600; color: var(--text-primary);
  }
  /* "editado" = veio de override, não do .env — sem isso não dá pra saber de onde o valor vem. */
  .tag {
    font-size: 10px; font-weight: 700; letter-spacing: 0.04em; text-transform: uppercase;
    color: var(--accent); background: var(--accent-dim);
    padding: 1px 6px; border-radius: var(--radius-full);
  }
  /* min-width:0 e o que importa aqui, nao so cosmetica: `.ajuda` e um <span>, e um <span> dentro de
     um flex column (`.txt`, `.ajuste`) tem `min-width:auto` por padrao — o navegador reserva a
     largura do texto INTEIRO sem quebrar, e a frase corta na borda do painel em vez de quebrar linha.
     Vale pra toda ajuda do arquivo (o bug ja existia antes dos sliders, so nao tinha aparecido com
     texto longo o bastante numa tela estreita). */
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; min-width: 0; }

  /* Veredito + "por quê?": mesma leitura do acordeão do formulário de modelo, com o <details>
     nativo no lugar do estado próprio. */
  .porque { font-size: 11px; }
  /* 24px é o alvo mínimo de toque (WCAG 2.5.8): sem o marcador nativo, o `summary` de 11px ficava
     com 17px de altura clicável. */
  .porque summary {
    display: flex; align-items: center; gap: var(--space-1);
    min-height: 24px; cursor: pointer; list-style: none;
  }
  .porque summary::-webkit-details-marker { display: none; }
  .vered { color: var(--text-muted); }
  .pq { color: var(--accent); }
  .porque summary:hover .pq { text-decoration: underline; }
  .motivo {
    margin: var(--space-1) 0 0;
    font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.5; max-width: 62ch;
  }

  input[type='text'], input[type='number'] {
    height: 40px;
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 16px;                 /* 16px evita o zoom automático do iOS ao focar */
    padding: 0 var(--space-3);
    outline: none;
    min-width: 0;
  }
  input:focus { border-color: var(--accent); }
  .campo-num { display: flex; align-items: center; gap: var(--space-2); }
  /* Largura de 5 dígitos: a caixa de 100px para um "30" parecia campo de texto vazio. */
  .campo-num input { width: 7ch; text-align: right; font-variant-numeric: tabular-nums; }
  .sufixo { font-size: var(--text-xs); color: var(--text-muted); }

  .mascara {
    display: inline-flex; align-items: baseline; gap: var(--space-2);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-secondary);
  }
  .mascara-nota { font-family: var(--font-ui); color: var(--success); font-size: 11px; }

  /* `.switch` é global (app.css) — vocabulário único de liga/desliga do app. */
</style>

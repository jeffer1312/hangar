<script lang="ts">
  // Aba "Contas liberadas": a política da máquina (`~/.hangar/orquestracao-contas.md`), editada por
  // conta. Esquerda: tudo que a máquina conhece, por provider, com interruptor; direita: a conta
  // escolhida (pode usar, pode trocar, modelos liberados). Vive fora do OrquestracaoSheet porque
  // Configurações → Orquestração mostra a mesma tela sem precisar de grupo.
  import * as m from '../paraglide/messages';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import { providerName } from '../lib/format';
  import { getOrqPolitica, putOrqConta } from '../lib/api';
  import { iniciais, politicaDe, type ContaInventario, type ModeloInventario, type OrqPolitica, type Papel, type Provider } from '../lib/orquestracao';

  interface Props {
    desktop: boolean;
    // Papéis do grupo aberto (se houver): mostra "quem usa esta conta agora".
    papeis?: Papel[];
    // Avisa quem monta (aba Papéis) que a política mudou e os seletores precisam reler.
    onSalvo?: (p: OrqPolitica) => void;
  }
  let { desktop, papeis = [], onSalvo = undefined }: Props = $props();

  const PROVIDERS: Provider[] = ['claude', 'codex', 'pi', 'kimi', 'omp'];

  let dados = $state<OrqPolitica | null>(null);
  let carregando = $state(false);
  let erro = $state('');
  let conflito = $state(false);
  let salvando = $state(false);
  let aviso = $state('');
  let sel = $state<{ provider: Provider; conta: string } | null>(null);

  // Formulário da conta escolhida
  let ligada = $state(false);
  let trocar = $state(true);
  let marcados = $state<string[]>([]);   // ids; ['*'] = todos
  let filtro = $state('');
  let idNovo = $state('');

  const inv = $derived(sel ? dados?.inventario.find((i) => i.provider === sel!.provider && i.conta === sel!.conta) ?? null : null);
  const pol = $derived(sel && dados ? politicaDe(dados.politica, sel.provider, sel.conta) : null);
  const todos = $derived(marcados.includes('*'));
  const catalogo = $derived(inv?.modelos ?? []);
  // Id marcado que o catálogo (reduzido) não conhece continua na lista, senão sumia da tela e
  // era apagado no próximo salvar.
  const lista = $derived.by((): ModeloInventario[] => {
    const ids = new Set(catalogo.map((x) => x.id));
    const extras = marcados.filter((id) => id !== '*' && !ids.has(id)).map((id) => ({ id }));
    return [...catalogo, ...extras];
  });
  const visiveis = $derived(filtro.trim()
    ? lista.filter((x) => (x.id + ' ' + (x.name ?? '')).toLowerCase().includes(filtro.trim().toLowerCase()))
    : lista);
  const nMarcados = $derived(todos ? lista.length : marcados.length);
  const quemUsa = $derived(sel ? papeis.filter((p) => p.provider === sel!.provider && p.conta === sel!.conta).map((p) => p.papel) : []);

  export async function recarregar() {
    carregando = true; erro = ''; conflito = false;
    try {
      dados = await getOrqPolitica();
      if (sel) carregarForm();
    } catch (e) {
      erro = (e as Error).message;
    } finally {
      carregando = false;
    }
  }
  $effect(() => { void recarregar(); });

  function carregarForm() {
    const p = pol;
    ligada = !!p;
    trocar = p?.trocar ?? true;
    marcados = p ? [...p.modelos] : ['*'];
    filtro = ''; idNovo = ''; aviso = '';
  }
  function escolher(i: ContaInventario) {
    sel = { provider: i.provider, conta: i.conta };
    carregarForm();
  }
  function alternar(id: string) {
    if (todos) { marcados = lista.map((x) => x.id).filter((x) => x !== id); return; }
    marcados = marcados.includes(id) ? marcados.filter((x) => x !== id) : [...marcados, id];
  }
  function adicionarId() {
    const id = idNovo.trim();
    if (!id || /[|\n\r]/.test(id)) return;
    if (todos) marcados = [...lista.map((x) => x.id), id];
    else if (!marcados.includes(id)) marcados = [...marcados, id];
    idNovo = '';
  }
  // Interruptor da lista: liga/desliga sem abrir o formulário.
  async function ligarRapido(i: ContaInventario, on: boolean) {
    if (!dados) return;
    const p = politicaDe(dados.politica, i.provider, i.conta);
    await gravar(i, { ligada: on, trocar: p?.trocar ?? true, modelos: p?.modelos ?? ['*'] });
  }
  async function salvar() {
    if (!inv) return;
    await gravar(inv, { ligada, trocar, modelos: marcados.length ? marcados : ['*'] });
  }
  async function gravar(i: ContaInventario, v: { ligada: boolean; trocar: boolean; modelos: string[] }) {
    if (!dados) return;
    salvando = true; erro = ''; aviso = ''; conflito = false;
    try {
      const r = await putOrqConta(i.conta, { provider: i.provider, apelido: i.apelido, mtime: dados.mtime, ...v });
      const pol2 = dados.politica.filter((c) => !(c.provider === i.provider && c.conta === i.conta));
      if (v.ligada) pol2.push({ conta: i.conta, provider: i.provider, apelido: i.apelido, modelos: v.modelos, trocar: v.trocar });
      dados = { ...dados, mtime: r.mtime, politica: pol2 };
      aviso = m.orqcfg_politica_salva();
      onSalvo?.(dados);
    } catch (e) {
      const err = e as Error & { status?: number };
      if (err.status === 409) conflito = true; else erro = err.message;
    } finally {
      salvando = false;
    }
  }
  // Pi manda o contexto como texto ("200k"), Kimi/motor como número de tokens.
  const contexto = (c: unknown) => typeof c === 'number' ? `${Math.round(c / 1000)}k` : typeof c === 'string' && c ? c : null;
  const ligadaNaLista = (i: ContaInventario) => !!dados && !!politicaDe(dados.politica, i.provider, i.conta);
  const travada = (i: ContaInventario) => { const p = dados ? politicaDe(dados.politica, i.provider, i.conta) : null; return !!p && !p.trocar; };
</script>

{#snippet listaContas()}
  <p class="oc-intro">{m.orqcfg_contas_intro()}</p>
  {#if conflito}
    <p class="oc-erro" role="alert">{m.orqcfg_arquivo_mudou()} <button type="button" class="oc-link" onclick={recarregar}>{m.orqcfg_recarregar()}</button></p>
  {/if}
  {#if erro}<p class="oc-erro" role="alert">{erro}</p>{/if}
  {#if carregando && !dados}
    <div aria-busy="true" aria-label={m.orqcfg_carregando()}>
      {#each [0, 1, 2, 3, 4] as k (k)}
        <div class="oc-item oc-skel">
          <span class="oc-skel-av"></span>
          <span class="oc-skel-txt"><span class="oc-skel-bar" style="width: 46%"></span><span class="oc-skel-bar oc-skel-bar--sub" style="width: 62%"></span></span>
          <span class="oc-skel-sw"></span>
        </div>
      {/each}
    </div>
  {/if}
  {#each PROVIDERS as prov (prov)}
    {@const itens = dados?.inventario.filter((i) => i.provider === prov) ?? []}
    {#if itens.length}
      <p class="oc-grupo"><ProviderGlyph provider={prov} size={13} /> {providerName(prov)}</p>
      {#each itens as i (i.provider + ':' + i.conta)}
        {@const on = ligadaNaLista(i)}
        <div class="oc-item" class:sel={sel?.provider === i.provider && sel?.conta === i.conta} class:off={!on}>
          <button type="button" class="oc-item-body" onclick={() => escolher(i)}>
            <span class="oc-av oc-av--{i.provider}" aria-hidden="true">{iniciais(i.apelido || i.conta)}</span>
            <span class="oc-item-txt">
              <span class="oc-nome">{i.apelido || i.conta}</span>
              <span class="oc-sub">{i.conta} · {i.modelos.length ? m.orqcfg_n_modelos({ n: i.modelos.length }) : ''}{i.reduced ? ' *' : ''}</span>
            </span>
            {#if travada(i)}<span class="oc-chip oc-chip--warn">{m.orqcfg_travada()}</span>
            {:else if !on}<span class="oc-chip oc-chip--bad">{m.orqcfg_proibida()}</span>{/if}
          </button>
          <input type="checkbox" role="switch" class="oc-sw" checked={on} disabled={salvando}
                 aria-label={i.apelido || i.conta}
                 onchange={(e) => ligarRapido(i, (e.currentTarget as HTMLInputElement).checked)} />
        </div>
      {/each}
    {/if}
  {/each}
{/snippet}

{#snippet formConta()}
  {#if inv}
    <div class="oc-head">
      <span class="oc-av oc-av--lg oc-av--{inv.provider}" aria-hidden="true">{iniciais(inv.apelido || inv.conta)}</span>
      <div>
        <h3 class="oc-h">{inv.apelido || inv.conta}</h3>
        <p class="oc-sub">{providerName(inv.provider)} · <code>{inv.conta}</code>{inv.modelos.length ? ` · ${m.orqcfg_n_modelos({ n: inv.modelos.length })}` : ''}</p>
      </div>
    </div>

    <label class="oc-row">
      <span><span class="oc-row-t">{m.orqcfg_pode_usar()}</span><span class="oc-row-d">{m.orqcfg_pode_usar_desc()}</span></span>
      <input type="checkbox" role="switch" class="oc-sw" bind:checked={ligada} />
    </label>
    <label class="oc-row">
      <span><span class="oc-row-t">{m.orqcfg_pode_trocar()}</span><span class="oc-row-d">{m.orqcfg_pode_trocar_desc()}</span></span>
      <input type="checkbox" role="switch" class="oc-sw" bind:checked={trocar} disabled={!ligada} />
    </label>

    <div class="oc-lab">
      <span>{m.orqcfg_modelos_liberados({ n: todos ? m.orqcfg_modelos_todos() : String(nMarcados), total: lista.length })}</span>
      <span>
        <button type="button" class="oc-link" onclick={() => (marcados = ['*'])}>{m.orqcfg_marcar_todos()}</button> ·
        <button type="button" class="oc-link" onclick={() => (marcados = [])}>{m.orqcfg_limpar()}</button>
      </span>
    </div>
    {#if lista.length > 6}
      <input class="field-input oc-filtro" type="search" bind:value={filtro} placeholder={m.orqcfg_filtrar_modelos()} aria-label={m.orqcfg_filtrar_modelos()} />
    {/if}
    <div class="oc-modelos">
      {#each visiveis as x (x.id)}
        <label class="oc-mrow">
          <input type="checkbox" checked={todos || marcados.includes(x.id)} onchange={() => alternar(x.id)} disabled={!ligada} />
          <span class="oc-mn"><b>{x.name ?? x.id}</b>{#if x.name}<code>{x.id}</code>{/if}</span>
          <small>{[contexto(x.context_length), x.efforts?.length ? `${x.efforts[0]}–${x.efforts[x.efforts.length - 1]}` : null].filter(Boolean).join(' · ')}</small>
        </label>
      {/each}
    </div>
    {#if inv.reduced}
      <p class="oc-hint" role="status">{m.orqcfg_lista_reduzida()}</p>
    {/if}
    <div class="oc-add">
      <input class="field-input" type="text" bind:value={idNovo} placeholder={m.orqcfg_add_modelo_placeholder()} aria-label={m.orqcfg_add_modelo_placeholder()}
             onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); adicionarId(); } }} />
      <button type="button" class="oc-link" onclick={adicionarId} disabled={!idNovo.trim()}>{m.orqcfg_adicionar()}</button>
    </div>

    <p class="oc-nota">{quemUsa.length ? m.orqcfg_quem_usa({ papeis: quemUsa.join(', ') }) : m.orqcfg_ninguem_usa()}</p>

    <div class="oc-acao">
      {#if conflito}
        <p class="oc-erro" role="alert">{m.orqcfg_arquivo_mudou()} <button type="button" class="oc-link" onclick={recarregar}>{m.orqcfg_recarregar()}</button></p>
      {:else if erro}<p class="oc-erro" role="alert">{erro}</p>
      {:else if aviso}<p class="oc-ok" role="status">{aviso}</p>{/if}
      <button type="button" class="oc-primary" onclick={salvar} disabled={salvando || conflito}>{salvando ? m.orqcfg_salvando() : m.orqcfg_salvar_politica()}</button>
      <p class="oc-rodape">{m.orqcfg_rodape_politica({ arquivo: dados?.arquivo ?? '' })}</p>
    </div>
  {:else}
    <div class="oc-vazio"><p>{m.orqcfg_escolha_conta()}</p></div>
  {/if}
{/snippet}

{#if desktop}
  <div class="oc-split">
    <aside class="oc-pane oc-esq">{@render listaContas()}</aside>
    <section class="oc-pane oc-dir">{@render formConta()}</section>
  </div>
{:else if sel}
  <button type="button" class="oc-back" onclick={() => (sel = null)}>‹ {m.orqcfg_aba_contas()}</button>
  {@render formConta()}
{:else}
  {@render listaContas()}
{/if}

<style>
  .oc-split { display: grid; grid-template-columns: minmax(300px, 5fr) 6fr; grid-template-rows: minmax(0, 1fr); height: 100%; }
  .oc-pane { min-height: 0; overflow-y: auto; display: flex; flex-direction: column; }
  /* O pane é flex-column pra ação colar no fundo; sem isto a lista de modelos era ESPREMIDA a
     uma linha pelo flex-shrink. */
  .oc-pane > :global(*) { flex: none; }
  .oc-esq { padding-right: var(--space-4); border-right: 1px solid var(--border-subtle); }
  .oc-dir { padding-left: var(--space-4); }
  .oc-intro, .oc-sub, .oc-rodape, .oc-hint, .oc-nota { font-size: var(--text-sm); color: var(--text-secondary); margin: 0 0 var(--space-3); }
  .oc-sub { margin: 2px 0 0; font-size: 12px; color: var(--text-muted); }
  .oc-grupo { display: flex; align-items: center; gap: 6px; font-size: 11px; text-transform: uppercase; letter-spacing: .06em; color: var(--text-muted); font-weight: 600; margin: var(--space-4) 0 var(--space-2); }
  .oc-item { display: flex; align-items: center; gap: var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); padding: 0 var(--space-3) 0 0; margin: 6px 0; background: transparent; }
  .oc-skel { padding: 10px var(--space-3); pointer-events: none; }
  .oc-skel-av, .oc-skel-bar, .oc-skel-sw {
    display: block; border-radius: 6px;
    background: linear-gradient(90deg, color-mix(in srgb, var(--text-muted) 14%, transparent) 25%, color-mix(in srgb, var(--text-muted) 28%, transparent) 50%, color-mix(in srgb, var(--text-muted) 14%, transparent) 75%);
    background-size: 200% 100%; animation: oc-shimmer 1.4s ease-in-out infinite;
  }
  .oc-skel-av { width: 34px; height: 34px; border-radius: 50%; flex: none; }
  .oc-skel-txt { flex: 1; }
  .oc-skel-bar { height: 12px; }
  .oc-skel-bar--sub { height: 9px; margin-top: 6px; }
  .oc-skel-sw { width: 38px; height: 22px; border-radius: 11px; flex: none; }
  @keyframes oc-shimmer { from { background-position: 200% 0; } to { background-position: -200% 0; } }
  .oc-item.sel { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .oc-item.off { opacity: .55; }
  .oc-item-body { flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-3); padding: 10px var(--space-3); text-align: left; color: inherit; }
  .oc-item-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .oc-nome { font-weight: 600; }
  .oc-item .oc-sub { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .oc-av { width: 34px; height: 34px; border-radius: 50%; display: grid; place-items: center; font-weight: 700; font-size: 12px; flex: none; background: var(--surface-raised); color: var(--text-secondary); }
  .oc-av--lg { width: 44px; height: 44px; font-size: 15px; }
  .oc-av--claude { color: #e3b341; } .oc-av--kimi { color: #79c0ff; } .oc-av--pi { color: #d2a8ff; } .oc-av--codex { color: var(--success); }
  .oc-chip { font-size: 10.5px; padding: 2px 7px; border-radius: 5px; font-weight: 600; flex: none; background: var(--surface-raised); }
  .oc-chip--bad { color: var(--error); }
  .oc-chip--warn { color: #e3b341; }
  .oc-sw { appearance: none; width: 38px; height: 22px; border-radius: 11px; background: var(--surface-raised); position: relative; flex: none; cursor: pointer; margin: 0; }
  .oc-sw::after { content: ""; position: absolute; top: 3px; left: 3px; width: 16px; height: 16px; border-radius: 50%; background: var(--text-muted); transition: transform 150ms var(--ease-out), background 150ms; }
  .oc-sw:checked { background: var(--success); }
  .oc-sw:checked::after { transform: translateX(16px); background: #fff; }
  .oc-sw:disabled { opacity: .5; cursor: default; }
  .oc-head { display: flex; align-items: center; gap: var(--space-3); margin-bottom: var(--space-3); }
  .oc-h { margin: 0; font-size: var(--text-lg); font-weight: 600; }
  .oc-row { display: flex; justify-content: space-between; align-items: center; gap: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); padding: 10px var(--space-3); margin: 6px 0; cursor: pointer; }
  .oc-row-t { display: block; } .oc-row-d { display: block; font-size: 12px; color: var(--text-muted); }
  .oc-lab { display: flex; justify-content: space-between; align-items: center; font-size: var(--text-sm); margin: var(--space-4) 0 var(--space-2); }
  .oc-link { color: var(--accent); font-size: 12px; background: none; padding: 0; }
  .oc-link:disabled { opacity: .5; }
  .oc-filtro { margin-bottom: 6px; }
  .oc-modelos { border: 1px solid var(--border-subtle); border-radius: var(--radius-md); max-height: 320px; overflow-y: auto; }
  .oc-mrow { display: flex; align-items: center; gap: 10px; padding: 8px 12px; border-bottom: 1px solid var(--border-subtle); cursor: pointer; }
  .oc-mrow:last-child { border-bottom: 0; }
  .oc-mn { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .oc-mn b { font-weight: 600; font-size: 13.5px; } .oc-mn code { font-size: 11px; color: var(--text-muted); }
  .oc-mrow small { color: var(--text-muted); font-size: 11px; white-space: nowrap; }
  .oc-add { display: flex; gap: var(--space-2); align-items: center; margin-top: 6px; }
  .oc-add .field-input { flex: 1; }
  .oc-nota { margin-top: var(--space-3); padding: 10px 12px; border-radius: var(--radius-md); background: var(--surface-raised); }
  .oc-acao { margin-top: auto; padding-top: var(--space-3); }
  .oc-primary { width: 100%; height: 48px; background: var(--accent); color: #fff; border-radius: var(--radius-md); font-weight: 600; font-size: var(--text-base); }
  .oc-primary:disabled { opacity: .5; }
  .oc-rodape { text-align: center; margin: 6px 0 0; font-size: 12px; }
  .oc-erro { color: var(--error); font-size: var(--text-sm); margin: 0 0 var(--space-2); }
  .oc-ok { color: var(--success); font-size: var(--text-sm); margin: 0 0 var(--space-2); }
  .oc-vazio { flex: 1; display: grid; place-items: center; color: var(--text-muted); font-size: var(--text-sm); padding: var(--space-6); }
  .oc-back { color: var(--accent); font-size: var(--text-sm); margin-bottom: var(--space-2); padding: 0; background: none; }
</style>

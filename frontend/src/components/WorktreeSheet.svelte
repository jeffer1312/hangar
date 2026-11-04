<script lang="ts">
  import { untrack } from 'svelte';
  import { basename, deleteWorktreeForServer, fmtBytes, fmtWhen, getWorktreeForServer, relativeTime, worktreeState,
           worktreeTitle, type Server, type WorktreeStatus } from '@hangar/core';
  import BottomSheet from './BottomSheet.svelte';
  import Spinner from './Spinner.svelte';
  import { worktreeStatus } from '../lib/worktreeStatus.svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { goToSession, sessionStateLabel, worktreeSizeLabel, worktreeStateColor, worktreeStateLabel } from '../lib/worktreeView';
  import * as m from '../paraglide/messages';

  interface Props { open: boolean; server: Server; path: string; onClose: () => void; onDeleted?: () => void }
  let { open, server, path, onClose, onDeleted }: Props = $props();

  let st = $state<WorktreeStatus | null>(null);
  let erro = $state('');
  let apagando = $state(false);
  let apagarBranch = $state(false);
  let confirmando = $state(false);
  let geracao = 0;

  // Só abrir e trocar de caminho releem: lista de servidores recarregada traz objeto novo do mesmo servidor.
  // Com o tamanho ainda em medição no backend, relê a cada 5 s, com teto.
  $effect(() => {
    if (!open) return;
    const p = path;
    const meu = ++geracao;
    st = null; erro = ''; apagarBranch = false; confirmando = false;
    apagando = false; copiado = false; erroCopia = false;
    const s = untrack(() => server);
    let releituras = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ler = () => getWorktreeForServer(s, p)
      .then((r) => {
        if (meu !== geracao) return;
        st = r; worktreeStatus.put(s.id, r);
        if (r.size_pending && releituras++ < 12) timer = setTimeout(ler, 5000);
      })
      .catch((e) => { if (meu === geracao) erro = e instanceof Error ? e.message : String(e); });
    void ler();
    return () => { geracao++; clearTimeout(timer); };
  });

  const estado = $derived(st ? worktreeState(st) : null);
  const veredito = $derived.by(() => {
    if (!st || !estado) return '';
    switch (estado) {
      case 'gone': return m.worktree_veredito_sumida();
      case 'session': return m.worktree_veredito_em_uso();
      case 'residual': return m.worktree_residual_warning();
      case 'dirty': return m.worktree_veredito_nao_commitado({ n: st.dirty });
      case 'merged': return m.worktree_veredito_mesclada();
      case 'detached': return m.worktree_veredito_sem_branch();
      case 'active': return m.worktree_veredito_andamento({ n: st.ahead });
    }
  });
  // Estado vivo vem do store compartilhado da lista, nunca de uma leitura por sessão.
  const sessoes = $derived(st ? st.sessions.map((nome) => ({
    nome, estado: sessionsStore.rows.find((r) => r.serverId === server.id && r.name === nome)?.state,
  })) : []);
  const perdeArquivos = $derived(st ? st.dirty + st.ignored.length : 0);
  // "Libera X" só com medida lida; parcial sai como mínimo ("≥").
  const tamanho = $derived(st && st.size != null && !st.size_pending && !st.size_error ? worktreeSizeLabel(st) : '');

  let copiado = $state(false);
  let erroCopia = $state(false);
  let timerCopia: ReturnType<typeof setTimeout> | undefined;
  async function copiarCaminho() {
    if (!st) return;
    erroCopia = false;
    try {
      await navigator.clipboard.writeText(st.path);
      copiado = true;
      clearTimeout(timerCopia);
      timerCopia = setTimeout(() => (copiado = false), 1500);
    } catch {
      erroCopia = true;
    }
  }

  function irPara(nome: string) {
    goToSession(server.id, nome);
    onClose();
  }

  async function apagar() {
    if (!st) return;
    // Capturado antes do await: trocar de worktree no meio zera `st`.
    const { repo, path: alvo } = st;
    const meu = geracao;
    apagando = true; erro = '';
    try {
      await deleteWorktreeForServer(server, { repo, path: alvo, confirm: !!st.residual || perdeArquivos > 0,
        delete_branch: !st.residual && apagarBranch, ...(st.residual ? { keep_branch: true } : {}) });
      worktreeStatus.drop(server.id, alvo);
      onDeleted?.();
      onClose();
    } catch (e) {
      if (meu === geracao) erro = e instanceof Error ? e.message : String(e);
    } finally {
      if (meu === geracao) apagando = false;
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={m.worktrees_titulo()}>
  <div class="sheet">
    {#if !st && !erro}
      <h2 class="title">{basename(path)}</h2>
      <div class="centro"><Spinner /></div>
    {:else if erro && !st}
      <h2 class="title">{basename(path)}</h2>
      <p class="erro" role="alert">{m.worktrees_erro({ motivo: erro })}</p>
    {:else if st && estado && confirmando}
      <h2 class="title">{m.worktree_apagar_titulo({ nome: worktreeTitle(st) })}</h2>
      <code class="caminho">{st.path}{st.branch ? ` · ${st.branch}` : ''}</code>
      {#if tamanho}<p class="libera">{m.worktree_libera({ tamanho })}</p>{/if}
      {#if st.residual}
        <p class="aviso">{m.worktree_residual_warning()}</p>
        {#if st.residual_entries == null}<p class="aviso" role="note">{m.worktree_residual_inventory_incomplete()}</p>{/if}
        {#if st.residual_entries != null}<p>{m.worktree_residual_entries({ n: st.residual_entries })}</p>{/if}
        <ul class="arquivos">{#each st.residual_files ?? [] as file (file)}<li>{file}</li>{/each}</ul>
      {/if}
      {#if perdeArquivos}
        <p class="secao secao--perde">{m.worktree_apagar_perde()}</p>
        <div class="caixa-perde">
          {#if st.dirty}<span class="forte">{m.worktree_n_nao_commitados({ n: st.dirty })}</span>{/if}
          <ul class="arquivos">
            {#each st.dirty_files ?? [] as f (f.path)}<li><span class="cod">{f.code}</span> {f.path}</li>{/each}
            {#each st.ignored as f (f)}<li><span class="cod">!!</span> {f}</li>{/each}
          </ul>
        </div>
      {/if}
      <p class="secao">{m.worktree_fica_guardado()}</p>
      <ul class="fica">
        {#if st.merged}
          <li>{m.worktree_apagar_branch_juntada({ branch: st.branch ?? '', base: st.base ?? '' })}</li>
        {:else if st.branch && st.ahead && !apagarBranch}
          <li>{m.worktree_fica_commits({ n: st.ahead, branch: st.branch })}</li>
        {/if}
        {#if st.closed}
          <li>{m.worktree_fica_conversas({ n: st.closed })}</li>
        {:else}
          <li>{m.worktree_apagar_conversas({ branch: st.main_branch ?? st.base ?? '' })}</li>
        {/if}
      </ul>
      {#if !st.residual && !st.merged && st.branch}
        <label class="marca"><input type="checkbox" bind:checked={apagarBranch} />
          <span class="marca-texto"><span>{m.worktree_apagar_branch_tambem({ n: st.ahead })}</span>
            {#if st.ahead}<span class="muted">{m.worktree_apagar_branch_perde({ n: st.ahead })}</span>{/if}</span></label>
      {/if}
      {#if erro}<p class="erro" role="alert">{erro}</p>{/if}
      <div class="acoes">
        <button type="button" class="cancelar" onclick={() => { erro = ''; confirmando = false; }}>{m.comum_cancelar()}</button>
        <button type="button" class="apagar" disabled={apagando} onclick={apagar}>
          {#if apagando}<Spinner />{/if}
          {perdeArquivos ? m.worktree_apagar_perder({ n: perdeArquivos }) : m.worktree_apagar()}
        </button>
      </div>
    {:else if st && estado}
      <div class="cabeca">
        <h2 class="title">{worktreeTitle(st)}</h2>
        <span class="chip" style:--cor={worktreeStateColor[estado]}>{worktreeStateLabel[estado]()}</span>
      </div>
      <div class="caminho-linha">
        <code class="caminho">{st.path}</code>
        <button type="button" class="copiar" onclick={copiarCaminho}>{copiado ? m.tool_copiado() : m.worktree_copiar_caminho()}</button>
      </div>
      {#if erroCopia}<p class="erro" role="alert">{m.toast_copiar_falhou()}</p>{/if}
      <p class="veredito">{veredito}</p>
      {#if erro}<p class="erro" role="alert">{m.worktrees_erro({ motivo: erro })}</p>{/if}
      {#if st.residual && st.residual_entries == null}
        <p class="aviso" role="note">{m.worktree_residual_inventory_incomplete()}</p>
      {:else if st.degraded && !st.residual}<p class="aviso" role="note">{m.worktree_leitura_incompleta()}</p>{/if}

      {#if sessoes.length}
        <p class="secao">{m.worktree_sessoes_aqui()}</p>
        <ul class="sessoes">
          {#each sessoes as s (s.nome)}
            <li>
              <span class="nome">{s.nome}</span>
              {#if s.estado}<span class="muted">{sessionStateLabel(s.estado)}</span>{/if}
              <button type="button" class="ir" aria-label={m.worktree_ir_sessao_nome({ nome: s.nome })} onclick={() => irPara(s.nome)}>{m.worktree_ir_sessao()} →</button>
            </li>
          {/each}
        </ul>
      {/if}

      <p class="secao">{m.worktree_detalhe_branch()}</p>
      <div class="branches">
        <div><span class="mono">{st.base ?? '—'}</span> <span class="muted">{m.worktree_detalhe_base({ n: st.behind ?? 0 })}</span></div>
        <div><span class="mono">{st.branch ?? m.worktree_sem_branch_rotulo()}</span> <span class="muted">{m.worktree_detalhe_so_dela({ n: st.ahead })}</span></div>
      </div>

      <dl class="fatos">
        <dt>{m.worktree_detalhe_criada()}</dt><dd>{st.created_at ? fmtWhen(st.created_at) : '—'}</dd>
        <dt>{m.worktree_detalhe_ultimo_commit()}</dt><dd>{st.last_commit ? fmtWhen(st.last_commit.at) : '—'}</dd>
        <dt>{m.worktree_detalhe_conversas()}</dt><dd>{st.closed}</dd>
        <dt>{m.worktree_detalhe_espaco()}</dt><dd class:aviso={st.size_error}>{worktreeSizeLabel(st)}</dd>
        {#if st.size_biggest}<dt>{m.worktree_detalhe_maior()}</dt><dd>{st.size_biggest.name} · {fmtBytes(st.size_biggest.bytes)}</dd>{/if}
      </dl>

      <p class="secao">{m.worktree_detalhe_commits()}</p>
      {#if st.commits?.length}
        <ul class="commits">
          {#each st.commits as c (c.sha)}
            <li><code>{c.sha.slice(0, 7)}</code><span class="assunto">{c.subject}</span><span class="muted">{relativeTime(c.at)}</span></li>
          {/each}
        </ul>
      {:else}
        <p class="muted">{m.worktree_detalhe_sem_commits()}</p>
      {/if}

      {#if st.dirty}
        <p class="secao">{m.worktree_detalhe_nao_commitado({ n: st.dirty })}</p>
        <ul class="arquivos">
          {#each st.dirty_files ?? [] as f (f.path)}<li><span class="cod">{f.code}</span> {f.path}</li>{/each}
        </ul>
      {/if}

      {#if st.sessions.length}
        <p class="aviso">{m.worktree_bloqueada({ nomes: st.sessions.join(', ') })}</p>
      {:else}
        <button type="button" class="apagar" onclick={() => { erro = ''; confirmando = true; }}>{m.worktree_apagar_reticencias()}</button>
      {/if}
    {/if}
  </div>
</BottomSheet>

<style>
  .sheet { padding: 16px; display: flex; flex-direction: column; gap: 8px; }
  .cabeca { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .title { font-size: 1.05rem; margin: 0; overflow-wrap: anywhere; }
  .chip { font-size: 0.75rem; padding: 2px 8px; border-radius: 999px; color: var(--cor);
          background: color-mix(in srgb, var(--cor) 14%, transparent); }
  .caminho { font-family: var(--font-mono); font-size: 0.8rem; color: var(--text-muted); overflow-wrap: anywhere; }
  .veredito { margin: 0; }
  .libera { margin: 0; color: var(--success-text); font-size: 0.9rem; }
  .secao { margin: 8px 0 0; font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted); }
  .secao--perde { color: var(--error); }
  .caixa-perde { display: flex; flex-direction: column; gap: 6px; padding: 10px 12px; border-radius: 10px;
                 border: 1px solid color-mix(in srgb, var(--error) 30%, transparent);
                 background: color-mix(in srgb, var(--error) 5%, transparent); }
  .forte { font-weight: 600; }
  .fica { margin: 0; padding-left: 18px; display: flex; flex-direction: column; gap: 4px; }
  .marca { display: flex; gap: 8px; align-items: flex-start; }
  .marca-texto { display: flex; flex-direction: column; gap: 2px; }
  .caminho-linha { display: flex; align-items: center; gap: 8px; }
  .caminho-linha .caminho { flex: 1; min-width: 0; }
  .copiar { flex: none; min-height: 32px; padding: 0 10px; border-radius: 8px; border: 1px solid var(--border-default); font-size: 0.8rem; }
  .muted { color: var(--text-muted); font-size: 0.85rem; }
  .mono { font-family: var(--font-mono); font-size: 0.85rem; overflow-wrap: anywhere; }
  .aviso { margin: 0; color: var(--warning-text); }
  .erro { margin: 0; color: var(--error); }
  .sessoes, .commits, .arquivos { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
  .sessoes li { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; padding: 8px 10px; border-radius: 10px;
                background: var(--surface-raised); }
  .sessoes .nome { font-weight: 600; }
  .ir { margin-left: auto; min-height: 36px; padding: 0 12px; border-radius: 8px; border: 1px solid var(--border-default); color: var(--accent-text); }
  .branches { display: flex; flex-direction: column; gap: 4px; }
  .fatos { display: grid; grid-template-columns: auto 1fr; gap: 4px 12px; margin: 8px 0 0; font-size: 0.9rem; }
  .fatos dt { color: var(--text-muted); }
  .fatos dd { margin: 0; overflow-wrap: anywhere; }
  .commits li { display: grid; grid-template-columns: auto 1fr auto; gap: 8px; align-items: baseline; font-size: 0.85rem; }
  .commits code { font-family: var(--font-mono); color: var(--text-muted); }
  .assunto { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .arquivos li { font-family: var(--font-mono); font-size: 0.8rem; overflow-wrap: anywhere; }
  .cod { color: var(--warning-text); display: inline-block; min-width: 2ch; }
  .centro { display: flex; justify-content: center; padding: 16px; }
  .acoes { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
  .cancelar { min-height: 40px; padding: 0 16px; border-radius: 8px; border: 1px solid var(--border-default); }
  .apagar { align-self: flex-end; display: inline-flex; align-items: center; gap: 8px; min-height: 40px; padding: 0 16px;
            border-radius: 8px; background: var(--error); color: #fff; border: 0; font-weight: 600; }
  .apagar:disabled { opacity: 0.6; }
</style>

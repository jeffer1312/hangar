<script lang="ts">
  // Saúde dos harnesses: uma linha por CLI (Claude Code, Codex, Pi, omp, Kimi) com o que o app
  // instalou nele — hooks, extensões, ponte de skills, login espalhado, contas — e um botão por
  // item fora do lugar que roda o conserto que já existe no servidor. Existe porque cada peça
  // dessas falha calada: sessão sem estado, skill que sumiu, CLI deslogado, e a pessoa só descobre
  // no meio do trabalho.
  import { listarHarnesses, consertarHarness, type Harness, type ItemHarness } from '../../lib/credenciais';
  import * as m from '../../paraglide/messages';
  import ProvedorIcone from '../icons/ProvedorIcone.svelte';
  import type { Server } from '../../lib/auth';

  interface Props { apiTarget: Server | null }
  let { apiTarget }: Props = $props();

  let lista = $state<Harness[]>([]);
  let carregando = $state(false);
  let erro = $state('');
  let consertando = $state<string | null>(null);
  let feito = $state('');

  // Alvo capturado na chamada e resposta descartada se ele mudou: trocar de servidor com a
  // requisição em voo não pode pintar a lista da máquina errada.
  let ger = 0;

  async function carregar() {
    const alvo = apiTarget;
    const g = ++ger;
    carregando = true; erro = '';
    try {
      const r = await listarHarnesses(alvo);
      if (g === ger) lista = r;
    } catch (e) { if (g === ger) erro = e instanceof Error ? e.message : String(e); }
    finally { if (g === ger) carregando = false; }
  }

  async function consertar(id: string) {
    if (consertando) return;
    const alvo = apiTarget;
    const g = ++ger;
    consertando = id; erro = ''; feito = '';
    try {
      const r = await consertarHarness(alvo, id);
      if (g !== ger) return;
      lista = r.harnesses;
      feito = r.feito;
    } catch (e) { if (g === ger) erro = e instanceof Error ? e.message : String(e); }
    finally { consertando = null; }
  }

  $effect(() => { void apiTarget; void carregar(); });

  // O código vem do servidor; a frase é daqui. Código desconhecido (backend mais novo que o app)
  // aparece cru em vez de sumir — sumir esconderia justamente o item que mudou.
  const TEXTOS: Record<string, (p: Record<string, string>) => string> = {
    skills_ok: (p) => (p.origem ? m.harness_skills_origem({ n: p.n ?? '', origem: p.origem }) : m.harness_skills_ok({ n: p.n ?? '' })),
    mcp_ok: (p) => m.harness_mcp_ok({ n: p.n ?? '', lista: p.lista ?? '' }),
    mcp_nenhum: () => m.harness_mcp_nenhum(),
    modelo_padrao: (p) => m.harness_modelo_padrao({ modelo: p.modelo ?? '' }),
    modelo_padrao_nenhum: () => m.harness_modelo_padrao_nenhum(),
    hooks_nenhum: () => m.harness_hooks_nenhum(),
    hooks_codex: (p) => m.harness_hooks_codex({ n: p.n ?? '', eventos: p.eventos ?? '' }),
    tmux_bloco_ok: () => m.harness_tmux_bloco_ok(),
    tmux_bloco_ausente: () => m.harness_tmux_bloco_ausente(),
    tmux_term_ok: (p) => m.harness_tmux_term_ok({ valor: p.valor ?? '' }),
    tmux_term_ruim: (p) => m.harness_tmux_term_ruim({ valor: p.valor ?? '' }),
    tmux_truecolor_ok: () => m.harness_tmux_truecolor_ok(),
    tmux_truecolor_ruim: () => m.harness_tmux_truecolor_ruim(),
    tmux_titulo_ok: () => m.harness_tmux_titulo_ok(),
    tmux_titulo_ruim: (p) => m.harness_tmux_titulo_ruim({ valor: p.valor ?? '' }),
    tmux_mouse_on: () => m.harness_tmux_mouse_on(),
    tmux_mouse_off: () => m.harness_tmux_mouse_off(),
    tmux_persist_on: () => m.harness_tmux_persist_on(),
    tmux_persist_off: () => m.harness_tmux_persist_off(),
    sem_ponte: () => m.harness_sem_ponte(),
    ponte_ausente: () => m.harness_ponte_ausente(),
    links_pendurados: (p) => m.harness_links_pendurados({ n: p.n ?? '', total: p.total ?? '' }),
    ponte_fora_da_config: (p) => m.harness_ponte_fora_da_config({ n: p.n ?? '', cli: p.cli ?? '' }),
    config_ilegivel: () => m.harness_config_ilegivel(),
    extensoes_ok: (p) => m.harness_extensoes_ok({ n: p.n ?? '' }),
    faltam: (p) => m.harness_faltam({ lista: p.lista ?? '' }),
    faltam_n: (p) => m.harness_faltam_n({ n: p.n ?? '' }),
    hooks_ok: (p) => m.harness_hooks_ok({ n: p.n ?? '' }),
    nenhuma_conta: () => m.harness_nenhuma_conta(),
    so_conta_padrao: () => m.harness_so_conta_padrao(),
    contas_ok: (p) => m.harness_contas_ok({ n: p.n ?? '', lista: p.lista ?? '' }),
    plugins_ok: (p) => m.harness_plugins_ok({ n: p.n ?? '', lista: p.lista ?? '' }),
    plugins_com_problema: (p) => m.harness_plugins_com_problema({ n: p.n ?? '', lista: p.lista ?? '' }),
    credenciais_ok: (p) => m.harness_credenciais_ok({ tem: p.tem ?? '' }),
    credenciais_faltam: (p) => m.harness_credenciais_faltam({ tem: p.tem ?? '', faltam: p.faltam ?? '' }),
    statusline_ok: () => m.harness_statusline_ok(),
    fullscreen_ok: () => m.harness_fullscreen_ok(),
    fullscreen_desligado: () => m.harness_fullscreen_desligado(),
    fullscreen_claude_desligado: () => m.harness_fullscreen_claude_desligado(),
    fullscreen_por_escolha: () => m.harness_fullscreen_por_escolha(),
    sem_statusline: () => m.harness_sem_statusline(),
  };
  const ROTULOS: Record<string, () => string> = {
    hooks: m.harness_item_hooks, contas: m.harness_item_contas, credenciais: m.harness_item_credenciais,
    plugins: m.harness_item_plugins, fullscreen: m.harness_item_fullscreen,
    mcp: m.harness_item_mcp, modelo: m.harness_item_modelo,
    bloco: m.harness_item_tmux_bloco, default_terminal: m.harness_item_tmux_term, truecolor: m.harness_item_tmux_truecolor,
    titulo: m.harness_item_tmux_titulo, mouse: m.harness_item_tmux_mouse, persistencia: m.harness_item_tmux_persist,
    skills: m.harness_item_skills, extensoes: m.harness_item_extensoes, statusline: m.harness_item_statusline,
  };
  function texto(i: ItemHarness): string { return (TEXTOS[i.codigo] ?? (() => i.codigo))(i.params); }
  function rotulo(i: ItemHarness): string { return (ROTULOS[i.id] ?? (() => i.id))(); }
</script>

<div class="hs">
  <div class="hs-cab">
    <p class="st-secao hs-titulo">{m.harness_titulo()}</p>
    <button type="button" class="hs-refresh" onclick={carregar} disabled={carregando}
      aria-label={m.arq_recarregar()}>{carregando ? '…' : '↻'}</button>
  </div>
  <p class="hs-leg">{m.harness_legenda()}</p>

  {#each lista as h (h.id)}
    <div class="hs-card" class:fora={!h.instalado}>
      <div class="hs-topo">
        <span class="hs-ponto" class:ok={h.instalado && h.itens.every((i) => i.ok !== false)}
              class:ruim={h.instalado && h.itens.some((i) => i.ok === false)} aria-hidden="true"></span>
        <ProvedorIcone tipo={h.id === 'claude' ? 'claude' : 'chave'}
          baseUrl={h.id === 'kimi' ? 'https://api.kimi.com' : h.id === 'codex' ? 'https://api.openai.com' : ''}
          iniciais={h.id === 'omp' ? 'ω' : h.id === 'pi' ? 'π' : h.id === 'tmux' ? '⌗' : h.nome.slice(0, 2).toUpperCase()} size={22} />
        <span class="hs-nome">{h.nome}</span>
        <span class="hs-versao">{h.instalado ? (h.versao || m.harness_instalado()) : m.harness_nao_instalado()}</span>
      </div>
      {#each h.itens as i (i.id)}
        <div class="hs-item">
          <span class="hs-marca" class:ok={i.ok === true && !i.info} class:ruim={i.ok === false} aria-hidden="true"
            >{i.info ? '·' : i.ok === true ? '✓' : i.ok === false ? '✕' : '?'}</span>
          <span class="hs-item-txt"><b>{rotulo(i)}</b> {texto(i)}</span>
          {#if i.conserto}
            <button type="button" class="hs-btn" onclick={() => consertar(i.conserto!)}
              disabled={consertando !== null}
              >{consertando === i.conserto ? '…'
                : i.conserto.startsWith('sync:') ? m.harness_sincronizar()
                : (i.ok === false ? m.harness_consertar() : m.harness_refazer())}</button>
          {/if}
        </div>
      {/each}
    </div>
  {/each}

  {#if feito}<p class="hs-aviso" role="status">{feito}</p>{/if}
  {#if erro}<p class="hs-aviso erro" role="alert">{erro}</p>{/if}
</div>

<style>
  .hs { container-type: inline-size; padding: var(--space-2) var(--space-3) var(--space-5); }
  .hs-cab { display: flex; align-items: center; gap: var(--space-2); margin: 0 0 var(--space-1); }
  .hs-titulo { flex: 1; margin: 0; }
  .hs-refresh { width: 28px; height: 28px; min-height: 0; min-width: 0; display: grid; place-items: center;
                border-radius: var(--radius-full); background: transparent; border: 1px solid var(--border-subtle);
                color: var(--text-secondary); }
  .hs-leg { margin: 0 0 var(--space-3); font-size: var(--text-xs); color: var(--text-muted); }
  .hs-card { padding: var(--space-2) var(--space-3); margin-bottom: var(--space-2);
             border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
             background: var(--surface-inset); }
  .hs-card.fora { opacity: 0.6; }
  .hs-topo { display: flex; align-items: center; gap: var(--space-2); }
  .hs-ponto { width: 8px; height: 8px; border-radius: 50%; background: var(--text-muted); }
  .hs-ponto.ok { background: var(--success, #3fb950); }
  .hs-ponto.ruim { background: var(--error); }
  .hs-nome { flex: 1; font-weight: 600; color: var(--text-primary); }
  .hs-versao { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  .hs-item { display: flex; align-items: center; gap: var(--space-2); margin-top: 6px; font-size: var(--text-sm); }
  .hs-marca { width: 16px; text-align: center; color: var(--text-muted); font-size: var(--text-xs); }
  .hs-marca.ok { color: var(--success, #3fb950); }
  .hs-marca.ruim { color: var(--error); }
  .hs-item-txt { flex: 1; min-width: 0; color: var(--text-secondary); overflow-wrap: anywhere; }
  .hs-item-txt b { color: var(--text-primary); font-weight: 600; }
  .hs-btn { flex-shrink: 0; min-height: 0; height: 26px; padding: 0 var(--space-2);
            font-size: var(--text-xs); border-radius: var(--radius-sm);
            background: var(--surface-raised); border: 1px solid var(--border-subtle); color: var(--text-primary); }
  .hs-aviso { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-secondary); }
  .hs-aviso.erro { color: var(--error); }
</style>

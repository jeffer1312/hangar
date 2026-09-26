<script lang="ts">
  // Como a sessão nasce (onde roda, modelo, esforço, permissão, motor, subagentes, Jev, perfil do
  // omp). Uma tela só para a folha de nova sessão e para o papel da orquestração: cada uma com a
  // sua cópia oferecia opções diferentes para a mesma sessão.
  import type { Snippet } from 'svelte';
  import { effortLevels, hasExecutionMode, permissionModes, type ModelOption, type Provider } from '@hangar/core';
  import Select from './Select.svelte';
  import { temEscolhaDeModelo, valorModelo } from '../lib/modelosPorConta';
  import { descricaoPermissao, rotuloPermissao } from '../lib/permissaoRotulo';
  import * as m from '../paraglide/messages';

  interface Props {
    provider: Provider;
    models: ModelOption[];
    engines: Record<string, { label?: string; model?: string }>;
    /** Ids dos campos: a folha de nova sessão mantém os de sempre (''), a orquestração usa 'orq-'. */
    idPrefix?: string;
    reducedList?: boolean;
    modelError?: string;
    modelLocked?: boolean;
    /** Retomar conversa: a sessão nasce do arquivo, então onde roda, modelo e o resto não valem. */
    resuming?: boolean;
    allowSubagent?: boolean;
    showJev?: boolean;
    headless: boolean;
    model: string;
    effort: string;
    permission: string;
    engine: string;
    subagent: string;
    jev: boolean;
    ompProfile: string;
    onEngineChange?: (engine: string) => void;
    onJevChange?: () => void;
    /** Entre "onde roda" e os campos de modelo (a folha põe ali o "continuar uma conversa"). */
    afterExecution?: Snippet;
    /** Depois de modelo/esforço/permissão (controle de contexto do Codex, teto e cota do papel). */
    afterChoices?: Snippet;
  }

  let {
    provider, models, engines, idPrefix = '', reducedList = false, modelError = '', modelLocked = false,
    resuming = false, allowSubagent = true, showJev = false,
    headless = $bindable(), model = $bindable(), effort = $bindable(), permission = $bindable(),
    engine = $bindable(), subagent = $bindable(), jev = $bindable(), ompProfile = $bindable(),
    onEngineChange, onJevChange, afterExecution, afterChoices,
  }: Props = $props();

  const id = (name: string) => `${idPrefix}${name}`;
  let differenceOpen = $state(false);
  let moreOpen = $state(false);

  const levels = $derived(effortLevels(provider, models, model));
  const modes = $derived(permissionModes(provider, headless));
  const hasEngine = $derived(provider === 'claude' && Object.keys(engines).length > 0);
  const hasSubagent = $derived(!resuming && allowSubagent && provider === 'claude' && !engine && models.length > 0);
  const effortLabel = $derived(provider === 'pi' || provider === 'omp' ? m.criar_raciocinio() : m.composer_esforco());
  const engineLabel = $derived(engine ? (engines[engine]?.label ?? engine) : m.criar_claude_sua_conta());
  const subagentLabel = $derived(subagent
    ? (models.find((mod) => valorModelo(mod) === subagent)?.name ?? subagent)
    : m.criar_subagente_padrao());

  function pickModel(v: string) {
    // Os níveis do Codex são por modelo: um nível que o modelo novo não lista não pode ficar.
    model = v;
    if (effort && !effortLevels(provider, models, v).includes(effort)) effort = '';
  }
</script>

{#if !resuming && hasExecutionMode(provider)}
  <!-- Logo abaixo da conta: decide se vai existir painel de terminal. -->
  <div class="field">
    <span class="field-label" id={id('modo-exec-rotulo')}>{m.criar_modo_exec()}</span>
    <div class="modos" role="group" aria-labelledby={id('modo-exec-rotulo')}>
      <button type="button" class="modo" class:on={!headless} aria-pressed={!headless} onclick={() => (headless = false)}>
        <span class="modo-radio" aria-hidden="true"></span>
        <span class="modo-nome">{m.criar_modo_exec_tmux()}</span>
        <span class="modo-resumo">{provider === 'codex' ? m.criar_modo_exec_tmux_resumo_codex() : m.criar_modo_exec_tmux_resumo()}</span>
      </button>
      <button type="button" class="modo" class:on={headless} aria-pressed={headless} onclick={() => (headless = true)}>
        <span class="modo-radio" aria-hidden="true"></span>
        <span class="modo-nome">{m.criar_modo_exec_headless()} <span class="modo-beta">{m.comum_beta()}</span></span>
        <span class="modo-resumo">{provider === 'codex' ? m.criar_modo_exec_headless_resumo_codex() : m.criar_modo_exec_headless_resumo()}</span>
      </button>
    </div>
    <button type="button" class="modo-diferenca" aria-expanded={differenceOpen} onclick={() => (differenceOpen = !differenceOpen)}>
      {m.criar_modo_exec_diferenca()}
      <span class="chevron" class:chevron--open={differenceOpen} aria-hidden="true">›</span>
    </button>
    {#if differenceOpen}
      <p class="hint">{provider === 'codex'
        ? (headless ? m.criar_modo_exec_headless_ajuda_codex() : m.criar_modo_exec_tmux_ajuda_codex())
        : (headless ? m.criar_modo_exec_headless_ajuda() : m.criar_modo_exec_tmux_ajuda())}</p>
    {/if}
  </div>
{/if}

{@render afterExecution?.()}

{#if !resuming}
  {#if provider === 'omp'}
    <div class="field">
      <label class="field-label" for={id('omp-profile')}>{m.criar_perfil_omp()}</label>
      <input id={id('omp-profile')} class="field-input" type="text" bind:value={ompProfile}
             placeholder={m.criar_perfil_omp_dica()} autocomplete="off" spellcheck="false" />
    </div>
  {/if}

  <!-- Lado a lado quando cabe: quem aperta é a largura do painel, não a da janela. -->
  <div class="trio">
    {#if temEscolhaDeModelo(provider)}
      <div class="field">
        <label class="field-label" for={id('model-pick')}>{m.composer_modelo()}</label>
        <Select id={id('model-pick')} class="field-input" ariaLabel={m.composer_modelo()} value={model} disabled={modelLocked}
          opcoes={[{ value: '', label: m.criar_padrao() },
                   // `default` do picker do Claude é o mesmo que o campo vazio (sem --model).
                   ...models.filter((mod) => mod.id !== 'default').map((mod) => ({
                     value: valorModelo(mod),
                     label: mod.name ?? mod.id,
                     // O id abre o hint quando o label é o nome: com nome repetido na lista (duas
                     // contas com o mesmo modelo) é o único lugar que os distingue.
                     hint: [(mod.name && mod.name !== mod.id) ? mod.id : null,
                            mod.provider,
                            mod.context ?? (mod.context_length ? `${Math.round(mod.context_length / 1000)}K` : null),
                            (mod.vision ?? mod.images) ? '👁' : null].filter(Boolean).join(' · ') }))]}
          onchange={pickModel} />
        {#if reducedList}
          <p class="model-hint" role="status" aria-live="polite" aria-atomic="true">{m.criar_lista_reduzida()}</p>
        {/if}
        {#if modelError}
          <p class="model-hint" role="alert">{m.criar_abre_padrao({ erro: modelError })}</p>
        {/if}
      </div>
    {/if}

    {#if levels.length > 0}
      <!-- Fora do if do modelo: o Kimi tem modelo mas não tem nível, e o Codex só tem níveis
           depois de o modelo estar escolhido. -->
      <div class="field">
        <label class="field-label" for={id('effort-pick')}>{effortLabel}</label>
        <Select id={id('effort-pick')} class="field-input" ariaLabel={effortLabel} value={effort}
          opcoes={[{ value: '', label: m.criar_padrao() }, ...levels.map((n) => ({ value: n, label: n }))]}
          onchange={(v) => (effort = v)} />
      </div>
    {/if}

    {#if modes.length > 0}
      <div class="field">
        <label class="field-label" for={id('perm-pick')}>{m.criar_permissao()}</label>
        <Select id={id('perm-pick')} class="field-input" ariaLabel={m.criar_permissao()} value={permission}
          opcoes={[{ value: '', label: m.criar_permissao_padrao() },
                   ...modes.map((n) => ({ value: n, label: rotuloPermissao(n), hint: descricaoPermissao(n) || undefined }))]}
          onchange={(v) => (permission = v)} />
      </div>
    {/if}
  </div>
{/if}

{@render afterChoices?.()}

{#if hasEngine || hasSubagent || showJev}
  <!-- O que quase ninguém muda fica recolhido, mas o resumo mostra o valor de cada um. -->
  <div class="mais" class:aberto={moreOpen}>
    <button type="button" class="mais-cab" aria-expanded={moreOpen} onclick={() => (moreOpen = !moreOpen)}>
      <span class="mais-nome">{m.criar_mais_opcoes()}</span>
      {#if !moreOpen}
        <span class="mais-resumo">
          {#if hasEngine}<span class="mais-pill">{m.comum_motor()} <em>{engineLabel}</em></span>{/if}
          {#if hasSubagent}<span class="mais-pill">{m.criar_mais_subagentes()} <em>{subagentLabel}</em></span>{/if}
          {#if showJev && jev}<span class="mais-pill">{m.criar_jev()}</span>{/if}
        </span>
      {/if}
      <span class="chevron" class:chevron--open={moreOpen} aria-hidden="true">›</span>
    </button>
    {#if moreOpen}
      <div class="mais-corpo">
        {#if hasEngine}
          <div class="field">
            <label class="field-label" for={id('engine-pick')}>{m.comum_motor()}</label>
            <Select id={id('engine-pick')} class="field-input" ariaLabel={m.comum_motor()} value={engine}
              opcoes={[{ value: '', label: m.criar_claude_sua_conta() },
                       ...Object.entries(engines).map(([nome, motor]) => ({ value: nome, label: motor.label ?? nome, hint: motor.model }))]}
              onchange={(v) => { engine = v; onEngineChange?.(v); }} />
          </div>
        {/if}
        {#if hasSubagent}
          <div class="field">
            <label class="field-label" for={id('subagent-pick')}>{m.criar_subagente()}</label>
            <Select id={id('subagent-pick')} class="field-input" ariaLabel={m.criar_subagente()} value={subagent}
              opcoes={[{ value: '', label: m.criar_subagente_padrao() },
                       ...models.filter((mod) => mod.id !== 'default').map((mod) => ({ value: valorModelo(mod), label: mod.name ?? mod.id }))]}
              onchange={(v) => (subagent = v)} />
            <p class="hint">{m.criar_subagente_ajuda()}</p>
          </div>
        {/if}
        {#if showJev}
          <div class="field">
            <label class="check">
              <input type="checkbox" bind:checked={jev} onchange={() => onJevChange?.()} />
              <span>{m.criar_jev()}</span>
            </label>
            <p class="hint">{m.criar_jev_ajuda()}</p>
          </div>
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .hint { font-size: var(--text-sm); color: var(--text-secondary); margin-bottom: var(--space-3); }
  .model-hint { margin: 6px 0 0; font-size: 12px; opacity: 0.75; }
  .chevron { color: var(--text-muted); transition: transform 180ms var(--ease-out); }
  .chevron--open { transform: rotate(90deg); }
  .check { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); color: var(--text-secondary); cursor: pointer; }
  .check input { accent-color: var(--accent); }

  .trio { display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: var(--space-3); }
  /* O .field já traz a margem de baixo; dentro do grid quem espaça é o gap. */
  .trio > .field { margin-bottom: var(--space-4); }
  .trio:empty { display: none; }

  .modos { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: var(--space-3); }
  .modo {
    position: relative; display: flex; flex-direction: column; align-items: flex-start; justify-content: flex-start;
    gap: 4px; padding: var(--space-3) calc(var(--space-3) + 24px) var(--space-3) var(--space-3);
    border-radius: var(--radius-md); border: 1px solid var(--border-default);
    background: var(--surface-raised, var(--bg-surface)); color: var(--text-secondary); text-align: left;
    transition: border-color 160ms ease-out, background 160ms ease-out, transform 160ms ease-out;
  }
  .modo:active { transform: scale(0.98); }
  .modo.on { border-color: var(--accent); background: var(--accent-dim); color: var(--text-primary); }
  .modo-nome { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .modo-resumo { font-size: var(--text-xs); line-height: 1.45; color: var(--text-muted); }
  .modo-radio {
    position: absolute; top: var(--space-3); right: var(--space-3); width: 16px; height: 16px;
    border-radius: 50%; border: 1.5px solid var(--text-muted);
  }
  .modo.on .modo-radio { border-color: var(--accent); background: radial-gradient(circle, var(--accent) 0 4px, transparent 4.5px); }
  .modo-beta {
    font-size: 10px; font-weight: 700; letter-spacing: 0.05em; text-transform: uppercase; padding: 2px 6px;
    border-radius: var(--radius-sm); background: var(--warning, #f2b64d); color: #1c1406;
  }
  .modo-diferenca {
    align-self: flex-start; display: inline-flex; align-items: center; gap: var(--space-1);
    padding: 2px 0; font-size: var(--text-xs); color: var(--accent);
  }

  .mais { margin-bottom: var(--space-4); border: 1px solid var(--border-default); border-radius: var(--radius-md); }
  .mais-cab {
    width: 100%; display: flex; align-items: center; gap: var(--space-3); min-height: 44px;
    padding: var(--space-2) var(--space-3); text-align: left;
  }
  .mais-nome { flex-shrink: 0; font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary); }
  .mais-resumo { flex: 1; min-width: 0; display: flex; flex-wrap: wrap; gap: var(--space-1); }
  .mais-pill {
    font-size: var(--text-xs); color: var(--text-muted); background: var(--surface-inset, var(--bg-surface));
    border-radius: 999px; padding: 2px var(--space-2); white-space: nowrap;
  }
  .mais-pill em { font-style: normal; color: var(--text-secondary); }
  .mais-cab .chevron { margin-left: auto; }
  .mais-corpo {
    display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); column-gap: var(--space-3);
    padding: var(--space-3) var(--space-3) 0; border-top: 1px solid var(--border-subtle);
  }
</style>

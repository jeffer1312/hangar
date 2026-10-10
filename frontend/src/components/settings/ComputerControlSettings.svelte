<script lang="ts">
  import type { Server } from '../../lib/auth';
  // Endereço, não frase: é o mesmo texto em qualquer idioma.
  const CLIPROXY_RELEASES = 'https://github.com/router-for-me/CLIProxyAPI/releases';
  import { getComputerControl, saveComputerControl, listComputerControlModels, createComputerControlTarget,
    installComputerControl, testComputerControlHost, getComputerControlWindowsSetup,
    type ComputerControlState, type ComputerControlTarget } from '../../lib/credenciais';
  import EscopoChip from './EscopoChip.svelte';
  import { copyText } from '../../lib/clipboard';
  import * as m from '../../paraglide/messages';

  let { apiTarget, onConfigureJev }: { apiTarget: Server | null; onConfigureJev: () => void } = $props();

  let current = $state<ComputerControlState | null>(null);
  let loading = $state(true);
  let saving = $state(false);
  let error = $state('');
  let savedMessage = $state('');

  let enabled = $state(false);
  let projectDir = $state('');
  let agentConfig = $state('');
  let preset = $state<'cliproxy' | 'custom'>('cliproxy');
  let url = $state('');
  let model = $state('');
  let effort = $state('');
  let newLlmKey = $state('');
  let models = $state<string[]>([]);
  let loadingModels = $state(false);
  let modelsError = $state('');

  const targetLabel = (t: ComputerControlTarget) =>
    `${t.name} · ${t.transport === 'local' ? m.computer_control_target_local() : t.host}`;

  let installing = $state(false);
  let installError = $state('');
  async function install() {
    if (installing) return;
    installing = true;
    installError = '';
    savedMessage = '';
    try {
      const r = await installComputerControl(apiTarget);
      fill(r);
      savedMessage = m.computer_control_installed({ tag: current?.installed_tag ?? '' });
      if (r.migration_skipped?.length) {
        installError = m.computer_control_migration_skipped({ files: r.migration_skipped.join(', ') });
      }
    } catch (e) {
      installError = e instanceof Error ? e.message : String(e);
    } finally {
      installing = false;
    }
  }

  // Formulário de alvo novo: vira um <nome>-agent.json na pasta dos alvos.
  let showNewTarget = $state(false);
  let newName = $state('');
  let newTransport = $state<'ssh' | 'local'>('ssh');
  let newHost = $state('');
  let newProxy = $state('');
  let newTimeout = $state('');
  let creating = $state(false);
  let newTargetError = $state('');
  let testing = $state(false);
  let testResult = $state<{ ok: boolean; detail: string } | null>(null);

  // Mesma regra do backend: sem nome, "administrator@delphi-03" vira "delphi-03".
  const derivedName = $derived(
    newHost.trim().split('@').pop()!.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^[-._]+|[-._]+$/g, '').slice(0, 41),
  );
  const finalName = $derived(newName.trim() || (newTransport === 'ssh' ? derivedName : ''));

  // Prompt pra colar num agente rodando no Windows: liga o SSH e autoriza a chave deste controlador.
  let setupPrompt = $state('');
  let setupError = $state('');
  let setupLoading = $state(false);
  let setupCopied = $state(false);
  async function loadSetupPrompt() {
    setupLoading = true;
    setupError = '';
    setupCopied = false;
    try {
      setupPrompt = (await getComputerControlWindowsSetup(apiTarget, newHost.trim())).prompt;
    } catch (e) {
      setupError = e instanceof Error ? e.message : String(e);
    } finally {
      setupLoading = false;
    }
  }
  async function copySetupPrompt() {
    try { await copyText(setupPrompt); setupCopied = true; }
    catch (e) { setupError = e instanceof Error ? e.message : String(e); }
  }

  async function testConnection() {
    if (testing || !newHost.trim()) return;
    testing = true;
    testResult = null;
    try {
      testResult = await testComputerControlHost(apiTarget, { host: newHost.trim(), proxy_command: newProxy.trim() });
    } catch (e) {
      testResult = { ok: false, detail: e instanceof Error ? e.message : String(e) };
    } finally {
      testing = false;
    }
  }

  async function createTarget() {
    if (creating) return;
    creating = true;
    newTargetError = '';
    const name = finalName;
    try {
      const s = await createComputerControlTarget(apiTarget, {
        project_dir: projectDir.trim(), name, transport: newTransport,
        host: newHost.trim(), proxy_command: newProxy.trim(),
        request_timeout: newTimeout ? Number(newTimeout) : null,
      });
      const created = s.targets.find((t) => t.name === name);
      current = s;
      if (created) agentConfig = created.path;
      showNewTarget = false;
      newName = newHost = newProxy = newTimeout = '';
      newTransport = 'ssh';
      testResult = null;
    } catch (e) {
      newTargetError = e instanceof Error ? e.message : String(e);
    } finally {
      creating = false;
    }
  }

  function fill(s: ComputerControlState) {
    current = s;
    enabled = s.enabled;
    projectDir = s.project_dir;
    agentConfig = s.agent_config;
    preset = s.llm_url === s.cliproxy.preset_url ? 'cliproxy' : 'custom';
    url = s.llm_url;
    model = s.llm_model;
    effort = s.llm_effort;
    newLlmKey = '';
  }

  async function load() {
    const target = apiTarget;
    loading = true;
    error = '';
    try {
      const s = await getComputerControl(target);
      if (target === apiTarget) fill(s);
    } catch (e) {
      if (target === apiTarget) error = e instanceof Error ? e.message : String(e);
    } finally {
      if (target === apiTarget) loading = false;
    }
  }
  $effect(() => { void apiTarget; void load(); });

  async function loadModels() {
    if (!current) return;
    loadingModels = true;
    modelsError = '';
    try {
      const r = await listComputerControlModels(apiTarget, preset === 'cliproxy'
        ? { llm_url: current.cliproxy.preset_url, use_cliproxy_key: true }
        : { llm_url: url, llm_key: newLlmKey || null, use_saved_key: !newLlmKey });
      models = r.models;
      if (!r.models.length) modelsError = m.computer_control_models_empty();
    } catch (e) {
      modelsError = e instanceof Error ? e.message : String(e);
    } finally {
      loadingModels = false;
    }
  }

  async function save(ev: SubmitEvent) {
    ev.preventDefault();
    if (!current || saving) return;
    saving = true;
    error = '';
    savedMessage = '';
    try {
      const s = await saveComputerControl(apiTarget, {
        enabled,
        mode: current.mode,
        project_dir: projectDir.trim(),
        agent_config: agentConfig,
        llm_url: preset === 'cliproxy' ? current.cliproxy.preset_url : url.trim(),
        llm_model: model.trim(),
        llm_effort: effort,
        llm_key: preset === 'custom' ? (newLlmKey || null) : null,
        use_cliproxy_key: preset === 'cliproxy',
      });
      fill(s);
      savedMessage = s.enabled
        ? m.computer_control_saved_on({ n: String(s.files.filter((f) => f.enabled).length) })
        : m.computer_control_saved_off();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }
</script>

<div class="cc">
  <p class="cc-title">{m.computer_control_title()} <EscopoChip escopo="servidor" /></p>
  <p>{m.computer_control_what()}</p>


  {#if error}<p class="err" role="alert">{error}</p>{/if}
  {#if loading}
    <p role="status">{m.comum_carregando()}</p>
  {:else if !current}
    <button type="button" class="action" onclick={load}>{m.lista_tentar_novamente()}</button>
  {:else}
    <div class="install">
      <p class="status">
        {!current.package_exists && current.mode !== 'local' ? m.computer_control_not_installed()
          : current.mode === 'package'
          ? m.computer_control_mode_package({ tag: current.installed_tag })
          : m.computer_control_mode_local({ dir: current.project_dir })}
      </p>
      <p class="hint">{!current.package_exists && current.mode !== 'local' ? m.computer_control_install_hint()
        : current.mode === 'package' ? m.computer_control_mode_package_hint() : m.computer_control_mode_local_hint()}</p>
      {#if current.agent_exe.exists}
        <p class="hint">{m.computer_control_agent_ok({ path: current.agent_exe.path, mb: (current.agent_exe.size / 1048576).toFixed(1) })}</p>
      {:else if current.package_exists || current.mode === 'local'}
        <p class="err" role="alert">
          {current.mode === 'package'
            ? m.computer_control_agent_missing_package({ path: current.agent_exe.path })
            : m.computer_control_agent_missing_local({ path: current.agent_exe.path })}
        </p>
      {/if}
      <button type="button" class="action" onclick={install} disabled={installing || saving} aria-busy={installing}>
        {installing ? m.computer_control_installing()
          : current.package_exists ? m.computer_control_update() : m.computer_control_install()}
      </button>
      {#if savedMessage}<p class="status" role="status">{savedMessage}</p>{/if}
      {#if current.package_exists && !current.targets.length}<p class="hint">{m.computer_control_install_next()}</p>{/if}
      {#if installError}<p class="err" role="alert">{installError}</p>{/if}
    </div>

    <!-- Chave em campo de texto mascarado por CSS, não type="password": com campo de senha no form o
         gerenciador do navegador ignora o autocomplete e enfia e-mail e senha salvos nos campos. -->
    <form onsubmit={save} autocomplete="off">
      <label class="check">
        <input type="checkbox" bind:checked={enabled} disabled={saving || installing || (!enabled && (!current.targets.length || (current.mode === 'package' && !current.package_exists)))} />
        <span>{m.computer_control_enable()}</span>
      </label>
      <p class="hint">{m.computer_control_enable_hint()}</p>

      {#if current.mode === 'local'}
        <label for="cc-dir">{m.computer_control_dir()}</label>
        <input id="cc-dir" name="cc-dir" autocomplete="off" bind:value={projectDir} disabled={saving || !enabled} spellcheck="false" />
      {/if}

      <label for="cc-agent">{m.computer_control_target()}</label>
      <div class="row">
        {#if current.targets.length}
          <select id="cc-agent" bind:value={agentConfig} disabled={saving || installing}>
            {#each current.targets as t (t.path)}<option value={t.path}>{targetLabel(t)}</option>{/each}
          </select>
        {:else}
          <input id="cc-agent" name="cc-agent" autocomplete="off" bind:value={agentConfig} disabled={saving || installing} spellcheck="false" />
        {/if}
        <button type="button" onclick={() => (showNewTarget = !showNewTarget)} aria-expanded={showNewTarget}
                disabled={saving || installing || (current.mode === 'package' && !current.package_exists)}>{m.computer_control_new_target()}</button>
      </div>
      <p class="hint">{current.targets.length ? m.computer_control_target_hint() : m.computer_control_target_empty()}</p>

      {#if showNewTarget}
        <div class="new-target">
          <div class="presets" role="radiogroup" aria-label={m.computer_control_target_kind()}>
            <button type="button" role="radio" aria-checked={newTransport === 'ssh'} class:on={newTransport === 'ssh'}
                    onclick={() => (newTransport = 'ssh')}>{m.computer_control_target_ssh()}</button>
            <button type="button" role="radio" aria-checked={newTransport === 'local'} class:on={newTransport === 'local'}
                    disabled={!current.local_available} onclick={() => (newTransport = 'local')}>{m.computer_control_target_local()}</button>
          </div>
          {#if !current.local_available}<p class="hint">{m.computer_control_target_local_hint()}</p>{/if}

          {#if newTransport === 'ssh'}
            <label for="cc-new-host">{m.computer_control_target_host()}</label>
            <div class="row">
              <input id="cc-new-host" name="cc-new-host" autocomplete="off" list="cc-ssh-hosts" bind:value={newHost}
                     oninput={() => (testResult = null)} placeholder="delphi-02" spellcheck="false" />
              <button type="button" onclick={testConnection} disabled={testing || !newHost.trim()} aria-busy={testing}>
                {testing ? m.computer_control_target_testing() : m.computer_control_target_test()}
              </button>
            </div>
            <datalist id="cc-ssh-hosts">{#each current.ssh_hosts as h (h)}<option value={h}></option>{/each}</datalist>
            <p class="hint">{m.computer_control_target_host_hint()}</p>
            {#if testResult}
              <p class={testResult.ok ? 'status' : 'err'} role="status">
                {testResult.ok ? m.computer_control_target_test_ok() : m.computer_control_target_test_fail({ detail: testResult.detail })}
              </p>
            {/if}
            <details class="help" open={testResult?.ok === false}>
              <summary>{m.computer_control_setup_title()}</summary>
              <p class="hint">{m.computer_control_setup_hint()}</p>
              {#if setupPrompt}
                <textarea class="setup-prompt" readonly rows="10" value={setupPrompt}></textarea>
                <div class="row">
                  <button type="button" onclick={copySetupPrompt}>{setupCopied ? m.computer_control_setup_copied() : m.computer_control_setup_copy()}</button>
                  <button type="button" onclick={loadSetupPrompt} disabled={setupLoading}>{m.computer_control_setup_regenerate()}</button>
                </div>
              {:else}
                <button type="button" class="action" onclick={loadSetupPrompt} disabled={setupLoading} aria-busy={setupLoading}>
                  {m.computer_control_setup_generate()}
                </button>
              {/if}
              {#if setupError}<p class="err" role="alert">{setupError}</p>{/if}
            </details>
          {/if}

          <label for="cc-new-name">{m.computer_control_target_name()}</label>
          <input id="cc-new-name" name="cc-new-name" autocomplete="off" bind:value={newName}
                 placeholder={derivedName || 'delphi-03'} spellcheck="false" />
          <p class="hint">{m.computer_control_target_name_hint()}</p>

          <details class="help">
            <summary>{m.computer_control_target_advanced()}</summary>
            {#if newTransport === 'ssh'}
              <label for="cc-new-proxy">{m.computer_control_target_proxy()}</label>
              <input id="cc-new-proxy" name="cc-new-proxy" autocomplete="off" bind:value={newProxy} spellcheck="false" />
              <p class="hint">{m.computer_control_target_proxy_hint()}</p>
            {/if}
            <label for="cc-new-timeout">{m.computer_control_target_timeout()}</label>
            <input id="cc-new-timeout" name="cc-new-timeout" autocomplete="off" type="number" min="1" max="600" bind:value={newTimeout} />
            <p class="hint">{m.computer_control_target_timeout_hint()}</p>
          </details>

          {#if newTargetError}<p class="err" role="alert">{newTargetError}</p>{/if}
          <button type="button" class="action" onclick={createTarget} disabled={creating || !finalName} aria-busy={creating}>
            {creating ? m.computer_control_target_creating() : m.computer_control_target_create()}
          </button>
        </div>
      {/if}

      <div class="jev-status">
        <p class="status">{m.computer_control_jev_title()}</p>
        <p class="hint">{current.jev_key_set
          ? m.computer_control_jev_shared({ tail: current.jev_key_tail })
          : m.computer_control_key_missing()}</p>
        <button type="button" onclick={onConfigureJev}>{m.jev_open_settings()}</button>
      </div>

      <p class="cc-title cc-sub">{m.computer_control_llm()}</p>
      <p class="hint">{m.computer_control_llm_hint()}</p>
      <div class="presets" role="radiogroup" aria-label={m.computer_control_llm()}>
        <button type="button" role="radio" aria-checked={preset === 'cliproxy'} class:on={preset === 'cliproxy'}
                disabled={saving || !enabled} onclick={() => { preset = 'cliproxy'; models = []; }}>{m.computer_control_preset_cliproxy()}</button>
        <button type="button" role="radio" aria-checked={preset === 'custom'} class:on={preset === 'custom'}
                disabled={saving || !enabled} onclick={() => { preset = 'custom'; models = []; }}>{m.computer_control_preset_custom()}</button>
      </div>

      {#if preset === 'cliproxy'}
        <p class="hint">
          {!current.cliproxy.installed ? m.computer_control_cliproxy_not_installed()
            : !current.cliproxy.running ? m.computer_control_cliproxy_stopped()
            : current.cliproxy.has_keys ? m.computer_control_cliproxy_key_ok() : m.computer_control_cliproxy_no_key()}
        </p>
        <details class="help" open={!current.cliproxy.installed || !current.cliproxy.running}>
          <summary>{m.computer_control_cliproxy_how()}</summary>
          <ol>
            <li>{m.computer_control_cliproxy_step_install()} <a href={CLIPROXY_RELEASES} target="_blank" rel="noreferrer">{CLIPROXY_RELEASES.replace('https://', '')}</a></li>
            <li>{m.computer_control_cliproxy_step_service()}</li>
            <li>{m.computer_control_cliproxy_step_panel()} <a href="http://127.0.0.1:8317/management.html" target="_blank" rel="noreferrer">http://127.0.0.1:8317/management.html</a></li>
            <li>{m.computer_control_cliproxy_step_key()}</li>
          </ol>
        </details>
      {:else}
        <label for="cc-url">{m.computer_control_url()}</label>
        <input id="cc-url" name="cc-url" autocomplete="off" bind:value={url} placeholder="https://…/v1/chat/completions" disabled={saving || !enabled} spellcheck="false" />
        <label for="cc-llm-key">{m.computer_control_llm_key()}</label>
        {#if current.llm_key_set && !current.cliproxy.key_is_cliproxy}
          <p class="hint">{m.computer_control_key_saved({ tail: current.llm_key_tail })}</p>
        {/if}
        <input id="cc-llm-key" name="cc-llm-key" class="secret" bind:value={newLlmKey} placeholder={m.computer_control_replace_key()}
               autocomplete="off" spellcheck="false" disabled={saving || !enabled} />
      {/if}

      <label for="cc-model">{m.computer_control_model()}</label>
      <div class="row">
        <input id="cc-model" name="cc-model" autocomplete="off" list="cc-models" bind:value={model} disabled={saving || !enabled} spellcheck="false" />
        <button type="button" onclick={loadModels} disabled={loadingModels || saving || !enabled} aria-busy={loadingModels}>
          {loadingModels ? m.computer_control_listing() : m.computer_control_list_models()}
        </button>
      </div>
      <datalist id="cc-models">{#each models as id (id)}<option value={id}></option>{/each}</datalist>
      {#if modelsError}<p class="err" role="alert">{modelsError}</p>
      {:else if models.length}<p class="hint">{m.computer_control_models_found({ n: String(models.length) })}</p>{/if}

      <label for="cc-effort">{m.computer_control_effort()}</label>
      <select id="cc-effort" bind:value={effort} disabled={saving || !enabled}>
        <option value="">{m.computer_control_effort_default()}</option>
        <option value="low">low</option>
        <option value="medium">medium</option>
        <option value="high">high</option>
      </select>

      <button class="action primary" type="submit" disabled={saving || installing || (enabled && (!agentConfig || (current.mode === 'package' && !current.package_exists)))} aria-busy={saving}>
        {saving ? m.computer_control_saving() : m.computer_control_save()}
      </button>
    </form>

    <p class="hint">{m.computer_control_where({ n: String(current.files.length) })}</p>
  {/if}
</div>

<style>
  .cc { display: flex; flex-direction: column; gap: var(--space-3); container-type: inline-size; }
  p { margin: 0; color: var(--text-secondary); font-size: var(--text-sm); line-height: 1.5; }
  .cc-title {
    display: flex; align-items: center; gap: var(--space-2);
    color: var(--text-muted); font-size: var(--label-size); font-weight: var(--label-weight);
    text-transform: uppercase; letter-spacing: var(--label-tracking);
  }
  .jev-status { display: flex; flex-direction: column; align-items: flex-start; gap: var(--space-2); margin-top: var(--space-4); }
  .cc-sub { margin-top: var(--space-4); }
  .help ol { margin: 0; padding-left: var(--space-5); color: var(--text-secondary); font-size: var(--text-sm); line-height: 1.5; }
  .hint { color: var(--text-muted); font-size: var(--text-xs); }
  .err { color: var(--error); }
  .status { color: var(--text-primary); font-weight: 600; }
  form { display: flex; flex-direction: column; gap: var(--space-2); }
  label { margin-top: var(--space-2); color: var(--text-primary); font-size: var(--text-sm); }
  .check { display: flex; align-items: center; gap: var(--space-2); margin-top: 0; font-weight: 600; }
  input:not([type='checkbox']), select {
    width: 100%; box-sizing: border-box; padding: var(--space-3); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); background: var(--surface-inset); color: var(--text-primary); font: inherit;
  }
  input:disabled, select:disabled { opacity: .6; }
  .row { display: flex; gap: var(--space-2); }
  .row input, .row select { flex: 1; min-width: 0; }
  .row button { flex-shrink: 0; white-space: nowrap; }
  .secret { -webkit-text-security: disc; }
  button, .action {
    padding: var(--space-3) var(--space-4); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-raised); color: var(--text-primary); font: inherit; font-size: var(--text-sm); cursor: pointer;
  }
  .action { align-self: flex-start; margin-top: var(--space-3); }
  .primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  button:disabled { opacity: .6; cursor: default; }
  .presets { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .install {
    display: flex; flex-direction: column; gap: var(--space-1);
    padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
  }
  .install .action { margin-top: var(--space-2); }
  .new-target {
    display: flex; flex-direction: column; gap: var(--space-2);
    padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
  }
  .new-target label { margin-top: var(--space-1); }
  .new-target .help { display: flex; flex-direction: column; gap: var(--space-2); }
  .setup-prompt {
    width: 100%; box-sizing: border-box; padding: var(--space-3); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); background: var(--surface-inset); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.5; resize: vertical;
  }
  .presets .on { border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
  .help summary { cursor: pointer; color: var(--text-secondary); font-size: var(--text-sm); }
  .help ol { margin-top: var(--space-2); }
  a { color: var(--accent); overflow-wrap: anywhere; }
  input:focus-visible, select:focus-visible, button:focus-visible, a:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
  @container (max-width: 420px) { .row { flex-direction: column; } }
</style>

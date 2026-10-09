<script lang="ts">
  import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
  import SegmentedPicker from '../SegmentedPicker.svelte';
  import {
    editTranscriptionProviderKey, editTranscriptionProviderTarget, getTranscriptionProvidersStatus, testTranscriptionProvider,
    moveTranscriptionProvider, parseTranscriptionProviders, transcriptionProviderKeepsKey, transcriptionProviderLabel,
    type TranscriptionProviderConfig, type TranscriptionProviderKind, type TranscriptionProviderStatus,
  } from '@hangar/core';
  import { intlLocale } from '../../lib/locale';
  import * as m from '../../paraglide/messages';

  // Ordem da lista = ordem de tentativa. O nome do serviço aparece só no item, nunca no rótulo da
  // capacidade (a seção continua se chamando "Transcrição").
  interface Props { store: ConfigServidorStore }
  let { store }: Props = $props();
  let destroyed = false;
  $effect(() => () => { destroyed = true; });
  const audioInputs: Record<string, HTMLInputElement | undefined> = {};
  let tests = $state<Record<string, { running: boolean; error: string; text: string }>>({});

  async function testAudio(id: string, file: File | undefined) {
    if (!file || tests[id]?.running) return;
    const server = store.alvo;
    const saved = JSON.stringify(salvos.get(id));
    tests[id] = { running: true, error: '', text: '' };
    try {
      const result = await testTranscriptionProvider(id, file, file.name, server);
      if (destroyed || store.alvo !== server || JSON.stringify(salvos.get(id)) !== saved) return;
      tests[id] = { running: false, error: result.aviso || '', text: result.text };
      releitura++;
    } catch (error) {
      if (!destroyed && store.alvo === server) tests[id] = { running: false,
        error: error instanceof Error ? error.message : m.erro_desconhecido(), text: '' };
    } finally {
      if (!destroyed && tests[id]?.running) tests[id] = { running: false, error: '', text: '' };
    }
  }
  const CHAVE = 'transcription_providers';

  const lista = $derived(parseTranscriptionProviders(store.valorBruto(CHAVE)));
  // Item salvo, por id: a máscara da chave devolvida intacta faz o backend manter a chave antiga.
  const salvos = $derived(new Map(parseTranscriptionProviders(store.campos[CHAVE]?.valor).map((p) => [p.id, p])));

  function gravar(nova: TranscriptionProviderConfig[]) { store.setRascunho(CHAVE, nova); }
  function atualizar(i: number, novo: TranscriptionProviderConfig) {
    gravar(lista.map((p, j) => (j === i ? novo : p)));
  }
  function remover(i: number) { gravar(lista.filter((_, j) => j !== i)); }
  function adicionar() {
    // crypto.randomUUID não existe fora de contexto seguro (PWA por http na rede local).
    const id = Math.random().toString(36).slice(2, 10);
    gravar([...lista, { id, kind: 'openai', name: '', base_url: '', api_key: '', model: '' }]);
  }

  // Espera por cota, lida do servidor editado. Relê quando os campos são trocados (abrir e salvar).
  let status = $state<TranscriptionProviderStatus[] | null>(null);
  let statusErro = $state('');
  let releitura = $state(0);
  $effect(() => {
    void store.campos;
    void releitura;
    if (store.carregando) return;
    let vivo = true;
    statusErro = '';
    getTranscriptionProvidersStatus(store.alvo)
      .then((r) => { if (vivo) status = r.providers; })
      .catch((e) => { if (vivo) statusErro = e instanceof Error ? e.message : String(e); });
    return () => { vivo = false; };
  });
  function emEspera(id: string): TranscriptionProviderStatus | null {
    const s = status?.find((x) => x.id === id);
    return s?.waiting_until && s.waiting_until * 1000 > Date.now() ? s : null;
  }
  const hora = (t: number) => new Date(t * 1000).toLocaleString(intlLocale(), { dateStyle: 'short', timeStyle: 'short' });
  const opcoesTipo = $derived([
    { v: 'openai' as TranscriptionProviderKind, label: m.native_voice_provider_kind_openai(), aria: m.native_voice_provider_kind_openai() },
    { v: 'elevenlabs' as TranscriptionProviderKind, label: m.native_voice_provider_kind_elevenlabs(), aria: m.native_voice_provider_kind_elevenlabs() },
    { v: 'whisper_cpp' as TranscriptionProviderKind, label: m.native_voice_provider_kind_whisper(), aria: m.native_voice_provider_kind_whisper() },
  ]);
</script>

<div class="servicos">
  <p class="ajuda">{m.native_voice_providers_help()}</p>
  {#if statusErro}
    <p class="aviso erro" role="alert">
      {m.native_voice_providers_status_failed({ error: statusErro })}
      <button class="link-btn" onclick={() => releitura++}>{m.config_server_tentar_de_novo()}</button>
    </p>
  {/if}
  {#if lista.length === 0}
    <p class="aviso">{m.native_voice_providers_empty()}</p>
  {:else}
    <ol class="lista">
      {#each lista as p, i (p.id)}
        {@const nome = transcriptionProviderLabel(p)}
        {@const espera = emEspera(p.id)}
        {@const salvo = salvos.get(p.id)}
        {@const mascara = salvo?.api_key || undefined}
        {@const mantem = transcriptionProviderKeepsKey(p, salvo)}
        <li class="item">
          <div class="item-cabeca">
            <span class="nome">{nome}</span>
            <div class="acoes">
              <button class="mini" onclick={() => gravar(moveTranscriptionProvider(lista, i, -1))} disabled={i === 0}
                aria-label={m.native_voice_provider_up({ name: nome })} title={m.native_voice_provider_up({ name: nome })}>↑</button>
              <button class="mini" onclick={() => gravar(moveTranscriptionProvider(lista, i, 1))} disabled={i === lista.length - 1}
                aria-label={m.native_voice_provider_down({ name: nome })} title={m.native_voice_provider_down({ name: nome })}>↓</button>
              <button class="mini" onclick={() => remover(i)}
                aria-label={m.native_voice_provider_remove({ name: nome })} title={m.native_voice_provider_remove({ name: nome })}>✕</button>
            </div>
          </div>
          {#if espera}
            <p class="espera" role="status">
              {m.native_voice_provider_waiting({ until: hora(espera.waiting_until!) })}{espera.reason ? `: ${espera.reason}` : ''}
            </p>
          {/if}
          <SegmentedPicker value={p.kind} options={opcoesTipo} ariaLabel={m.voz_servico_tipo()}
            onPick={(v) => { if (v !== p.kind) atualizar(i, editTranscriptionProviderTarget(p, { kind: v }, salvo)); }} />
          <div class="campos">
            {#if p.kind === 'whisper_cpp'}
              <p class="ajuda local-help">{m.native_voice_provider_local_help({ server: store.alvo?.label || m.native_voice_provider_current_server() })}</p>
              <a class="link-btn" href="https://github.com/ggml-org/whisper.cpp" target="_blank" rel="noopener noreferrer">{m.native_voice_provider_official()}</a>
              {#each [
                { field: 'executable_path' as const, label: m.native_voice_provider_executable(), placeholder: 'whisper-server' },
                { field: 'model_path' as const, label: m.native_voice_provider_model_file(), placeholder: 'ggml-small.bin' },
                { field: 'language' as const, label: m.native_voice_provider_language(), placeholder: 'pt' },
                { field: 'converter_path' as const, label: m.native_voice_provider_converter(), placeholder: 'ffmpeg' },
              ] as item (item.field)}
                <label class="campo">
                  <span class="rot">{item.label}</span>
                  <input name={item.field} type="text" autocomplete="off" autocapitalize="off" spellcheck={false}
                    value={p[item.field] || ''} placeholder={item.placeholder}
                    oninput={(e) => atualizar(i, { ...p, [item.field]: e.currentTarget.value })} />
                </label>
              {/each}
              {@const local = status?.find((s) => s.id === p.id)}
              {#if local?.state}
                <p class="ajuda" role="status">{local.state === 'ready' ? m.native_voice_provider_local_ready()
                  : local.state === 'starting' ? m.native_voice_provider_local_starting()
                  : local.state === 'failed' ? m.native_voice_provider_local_failed() : m.native_voice_provider_local_not_started()}</p>
                {#if local.error}<p class="aviso erro" role="alert">{local.error}</p>{/if}
              {/if}
            {:else}
            {#if p.kind === 'openai'}
              <label class="campo">
                <span class="rot">{m.native_voice_provider_endpoint()}</span>
                <input type="url" autocomplete="off" value={p.base_url} placeholder="https://api.groq.com/openai/v1"
                  oninput={(e) => atualizar(i, editTranscriptionProviderTarget(p, { base_url: e.currentTarget.value }, salvo))} />
              </label>
            {/if}
            <div class="campo">
              <label class="rot" for={`tp-key-${p.id}`}>{p.kind === 'openai' ? m.native_voice_provider_key_optional() : m.native_voice_provider_key()}</label>
              <!-- O campo nunca recebe a máscara; ela fica ao lado. Password: a chave não aparece na tela. -->
              {#if mascara && p.api_key === mascara}
                <span class="mascara">{mascara} <span class="mascara-nota">{m.config_server_configurada()}</span></span>
              {/if}
              <input id={`tp-key-${p.id}`} type="password" autocomplete="new-password" autocapitalize="off" spellcheck={false}
                value={p.api_key === mascara ? '' : p.api_key}
                placeholder={mascara ? m.config_motores_colar_nova() : m.config_motores_colar()}
                oninput={(e) => atualizar(i, editTranscriptionProviderKey(p, e.currentTarget.value, mantem ? mascara : undefined))} />
              {#if p.kind === 'elevenlabs' && !p.api_key}<span class="falta" role="alert">{m.native_voice_provider_missing_key()}</span>{/if}
            </div>
            <label class="campo">
              <span class="rot">{m.native_voice_provider_model()}</span>
              <input type="text" autocomplete="off" value={p.model} placeholder={p.kind === 'elevenlabs' ? 'scribe_v2' : 'whisper-large-v3'}
                oninput={(e) => atualizar(i, { ...p, model: e.currentTarget.value })} />
            </label>
            {/if}
          </div>
          <input type="file" accept="audio/*,.webm,.m4a" hidden bind:this={audioInputs[p.id]}
            onchange={(e) => { void testAudio(p.id, e.currentTarget.files?.[0]); e.currentTarget.value = ''; }} />
          <button class="btn" disabled={!salvo || JSON.stringify(p) !== JSON.stringify(salvo) || tests[p.id]?.running}
            onclick={() => audioInputs[p.id]?.click()}>{tests[p.id]?.running ? m.native_voice_provider_test_running() : m.native_voice_provider_test_audio()}</button>
          {#if !salvo || JSON.stringify(p) !== JSON.stringify(salvo)}<p class="ajuda">{m.native_voice_provider_test_saved()}</p>{/if}
          {#if tests[p.id]?.error}<p class="aviso erro" role="alert">{tests[p.id].error}</p>{/if}
          {#if tests[p.id]?.text}<p class="ajuda" role="status">{m.native_voice_provider_test_success()}</p><p class="resultado">{tests[p.id].text}</p>{/if}
        </li>
      {/each}
    </ol>
  {/if}
  <button class="btn" onclick={adicionar}>{m.native_voice_provider_add()}</button>
</div>

<style>
  /* Container query, nunca media query: quem aperta a linha é a largura do painel. */
  .servicos { container-type: inline-size; display: flex; flex-direction: column; gap: var(--space-2); margin-top: var(--space-2); }
  .lista { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  .item { display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-3) 0; border-bottom: 1px solid var(--border-subtle); }
  .item-cabeca { display: flex; align-items: center; gap: var(--space-2); }
  .nome { flex: 1 1 auto; min-width: 0; font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); overflow-wrap: anywhere; }
  .acoes { display: flex; gap: 4px; flex-shrink: 0; }
  .mini {
    width: 32px; height: 32px; border-radius: var(--radius-md); background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm);
  }
  .mini:disabled { opacity: 0.35; }
  .espera { margin: 0; font-size: var(--text-xs); color: var(--warning, var(--text-muted)); }
  .campos { display: grid; grid-template-columns: 1fr; gap: var(--space-2); }
  .local-help { grid-column: 1 / -1; }
  .resultado { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
  @container (min-width: 520px) { .campos { grid-template-columns: 1fr 1fr; } }
  .campo { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .rot { font-size: var(--text-xs); font-weight: 600; color: var(--text-secondary); }
  .campo input {
    height: 36px; padding: 0 10px; min-width: 0;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-inset); color: var(--text-primary);
    font-family: var(--font-ui); font-size: var(--text-sm);
  }
  .mascara { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); overflow-wrap: anywhere; }
  .mascara-nota { font-family: var(--font-ui); color: var(--success); }
  .falta { font-size: var(--text-xs); color: var(--warning, var(--error)); }
  .ajuda { margin: 0; font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; }
  .aviso { font-size: var(--text-sm); color: var(--text-muted); margin: 0; }
  .aviso.erro { color: var(--error); }
  .link-btn { margin-left: var(--space-2); padding: 0; background: none; border: none; color: var(--accent); font-size: inherit; }
  .btn {
    align-self: flex-start; height: 40px; padding: 0 var(--space-4); border-radius: var(--radius-md);
    background: var(--surface-raised); color: var(--text-primary); font-size: var(--text-sm); font-weight: 600;
  }
</style>

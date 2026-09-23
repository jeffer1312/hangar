<script lang="ts">
  // Editor da fileira de atalhos configurável: lista ordenada única (nativos + customizados),
  // subir/descer, remover, formulário de adicionar/editar com ícone curado ou emoji, e
  // "restaurar padrão" que apaga o override.
  // O estado salvo mora no servidor (runtime_config.shortcuts) via lib/shortcuts.svelte.ts.
  import * as m from '../../paraglide/messages';
  import {
    defaultShortcuts, getCommands, getSessions,
    type Shortcut, type ShortcutInternalAction, type ShortcutSendText, type ShortcutShell,
  } from '@hangar/core';
  import { loadShortcuts, shortcutsFor, saveShortcuts } from '../../lib/shortcuts.svelte';
  import ShortcutIcon, { GLYPHS } from '../icons/ShortcutIcon.svelte';
  import type { Server } from '../../lib/auth';

  interface Props {
    apiTarget: Server | null;
  }
  let { apiTarget }: Props = $props();
  const serverId = $derived(apiTarget?.id ?? null);

  let list = $state<Shortcut[]>([]);
  let loading = $state(true);
  let loadError = $state(false);
  let saving = $state(false);
  let saved = $state(false);
  let saveError = $state('');
  let dirty = $state(false);

  async function load() {
    // Servidor fixado na entrada: se o alvo trocar durante a busca, a resposta velha não pode
    // sobrescrever a lista (e a edição) do servidor novo.
    const target = serverId;
    loading = true;
    loadError = false;
    try {
      await loadShortcuts(target);
      if (target !== serverId) return;
      list = shortcutsFor(target).map((s) => ({ ...s }));
      dirty = false;
    } catch (err) {
      if (target !== serverId) return;
      console.error('shortcuts load error:', err);
      loadError = true;
    } finally {
      if (target === serverId) loading = false;
    }
  }
  $effect(() => { serverId; void load(); });

  async function save() {
    if (saving) return;
    saving = true;
    saveError = '';
    try {
      await saveShortcuts(list, serverId);
      dirty = false;
      saved = true;
      setTimeout(() => (saved = false), 2500);
    } catch (e) {
      // Erro de validação do backend chega como veio ("shortcuts: item 2 …").
      saveError = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function restoreDefaults() {
    if (saving) return;
    saving = true;
    saveError = '';
    try {
      await saveShortcuts(null, serverId);
      list = defaultShortcuts();
      dirty = false;
    } catch (e) {
      saveError = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  // ── Lista ───────────────────────────────────────────────────────────────────
  const INTERNAL_LABEL: Record<ShortcutInternalAction, () => string> = {
    terminal: m.ctx_terminal,
    modo: m.atalhos_interno_modo,
    navegador: m.ctx_navegador,
    anexos: m.ctx_anexos,
    rodar: m.ctx_rodar,
  };
  const INTERNAL_ICON: Record<ShortcutInternalAction, string> = {
    terminal: 'glifo:terminal', modo: 'glifo:git', navegador: 'glifo:globe',
    anexos: 'glifo:folder', rodar: 'glifo:play',
  };
  const missingNatives = $derived(
    (Object.keys(INTERNAL_LABEL) as ShortcutInternalAction[]).filter(
      (a) => !list.some((s) => s.type === 'internal' && s.action === a)));

  function move(i: number, delta: -1 | 1) {
    const j = i + delta;
    if (j < 0 || j >= list.length) return;
    const next = [...list];
    [next[i], next[j]] = [next[j], next[i]];
    list = next;
    dirty = true;
  }

  // ── Arrastar pra reordenar. HTML5 DnD não responde ao toque em tablet (regra do repo), então
  // os botões ↑/↓ ficam — são a alternativa exigida pela WCAG 2.2 SC 2.5.7, não redundância. ──
  let dragIdx = $state<number | null>(null);
  // A lista se reordena durante o arrasto; cancelado (Esc, soltar fora) ele volta a como estava.
  let beforeDrag: { list: Shortcut[]; dirty: boolean } | null = null;
  function dragStart(e: DragEvent, i: number) {
    dragIdx = i;
    beforeDrag = { list, dirty };
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', String(i));
    }
  }
  function dragOver(e: DragEvent, i: number) {
    e.preventDefault();       // sem isto o drop é recusado e o arrasto "volta"
    if (dragIdx === null || dragIdx === i) return;
    const next = [...list];
    const [item] = next.splice(dragIdx, 1);
    next.splice(i, 0, item);
    list = next;
    dragIdx = i;
    dirty = true;
  }
  function dragEnd(e: DragEvent) {
    if (e.dataTransfer?.dropEffect === 'none' && beforeDrag) {
      list = beforeDrag.list;
      dirty = beforeDrag.dirty;
    }
    beforeDrag = null;
    dragIdx = null;
  }
  function remove(i: number) {
    list = list.filter((_, k) => k !== i);
    dirty = true;
  }
  function restoreNative(a: ShortcutInternalAction) {
    list = [...list, { id: a, type: 'internal', action: a }];
    dirty = true;
  }

  // ── Formulário (adicionar/editar customizado) ───────────────────────────────
  let formOpen = $state(false);
  let editingIdx = $state<number | null>(null);   // índice na lista; null = novo
  let fType = $state<'send_text' | 'shell'>('send_text');
  let fLabel = $state('');
  let fGlyph = $state('bolt');
  let fEmoji = $state('');
  let fContent = $state('');
  let fSendDirect = $state(true);
  let fConfirm = $state(false);

  function openNew() {
    editingIdx = null;
    fType = 'send_text'; fLabel = ''; fGlyph = 'bolt'; fEmoji = '';
    fContent = ''; fSendDirect = true; fConfirm = false;
    formOpen = true;
  }
  function openEdit(i: number) {
    const s = list[i];
    if (s.type === 'internal') return;
    editingIdx = i;
    fType = s.type;
    fLabel = s.label;
    fContent = s.type === 'shell' ? s.command : s.text;
    fSendDirect = s.type === 'send_text' ? s.send_direct !== false : true;
    fConfirm = s.confirm === true;
    if (s.icon?.startsWith('emoji:')) { fEmoji = s.icon.slice(6); fGlyph = 'bolt'; }
    else { fEmoji = ''; fGlyph = s.icon?.startsWith('glifo:') ? s.icon.slice(6) : 'bolt'; }
    formOpen = true;
  }
  const formValid = $derived(!!fLabel.trim() && !!fContent.trim());
  function submitForm() {
    if (!formValid) return;
    const icon = fEmoji.trim() ? `emoji:${fEmoji.trim()}` : `glifo:${fGlyph}`;
    const base = { label: fLabel.trim(), icon, ...(fConfirm ? { confirm: true } : {}) };
    const shortcut: ShortcutSendText | ShortcutShell = fType === 'shell'
      ? { id: formId(), type: 'shell', command: fContent.trim(), ...base }
      : { id: formId(), type: 'send_text', text: fContent.trim(),
          ...(fSendDirect ? {} : { send_direct: false }), ...base };
    if (editingIdx === null) list = [...list, shortcut];
    else list = list.map((s, i) => (i === editingIdx ? shortcut : s));
    dirty = true;
    formOpen = false;
  }
  function formId(): string {
    if (editingIdx !== null) return list[editingIdx].id;
    return `a-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
  }

  // ── Sugestão de skill (datalist): comandos de uma sessão viva do servidor ativo. Sem sessão,
  // o campo fica livre — a sugestão é conforto, não requisito. ─────────────────────────────────
  let suggestions = $state<string[]>([]);
  $effect(() => {
    if (!formOpen || fType !== 'send_text' || suggestions.length) return;
    void (async () => {
      try {
        const sessions = await getSessions();
        const alive = sessions.find((s) => s.state !== 'dead');
        if (!alive) return;
        const cmds = await getCommands(alive.name);
        suggestions = cmds.map((c) => c.display ?? `/${c.name}`);
      } catch { /* sem sugestão, campo livre */ }
    })();
  });
</script>

<div class="at">
  <p class="sub">{m.atalhos_sub()}</p>

  {#if loading}
    <p class="estado">{m.comum_carregando()}</p>
  {:else if loadError}
    <p class="estado erro">{m.atalhos_erro_carregar()}</p>
    <button class="btn" onclick={() => void load()}>{m.config_server_tentar_de_novo()}</button>
  {:else}
    {#if list.length === 0}
      <p class="estado">{m.atalhos_vazio()}</p>
    {/if}
    <ul class="linhas">
      {#each list as s, i (s.id)}
        <li class="linha" class:arrastando={dragIdx === i} draggable="true"
            ondragstart={(e) => dragStart(e, i)} ondragover={(e) => dragOver(e, i)}
            ondragend={dragEnd}>
          <span class="alca" aria-hidden="true">⠿</span>
          <span class="ico"><ShortcutIcon icon={s.type === 'internal' ? INTERNAL_ICON[s.action] : s.icon} /></span>
          <span class="txt">
            <span class="rotulo">{s.type === 'internal' ? INTERNAL_LABEL[s.action]() : s.label}</span>
            {#if s.type !== 'internal'}
              <span class="detalhe">{s.type === 'shell' ? s.command : s.text}</span>
            {/if}
          </span>
          <span class="acoes">
            {#if s.type !== 'internal'}
              <button class="mini" onclick={() => openEdit(i)} aria-label={m.atalhos_editar()}>✎</button>
            {/if}
            <button class="mini" onclick={() => move(i, -1)} disabled={i === 0} aria-label={m.atalhos_subir()}>↑</button>
            <button class="mini" onclick={() => move(i, 1)} disabled={i === list.length - 1} aria-label={m.atalhos_descer()}>↓</button>
            <button class="mini" onclick={() => remove(i)} aria-label={m.atalhos_remover()}>✕</button>
          </span>
        </li>
      {/each}
    </ul>

    {#if missingNatives.length}
      <div class="repor">
        <span>{m.atalhos_repor()}</span>
        {#each missingNatives as a (a)}
          <button class="chip" onclick={() => restoreNative(a)}>+ {INTERNAL_LABEL[a]()}</button>
        {/each}
      </div>
    {/if}

    {#if formOpen}
      <div class="form">
        <label class="campo">
          <span>{m.atalhos_tipo()}</span>
          <select bind:value={fType} disabled={editingIdx !== null}>
            <option value="send_text">{m.atalhos_tipo_send()}</option>
            <option value="shell">{m.atalhos_tipo_shell()}</option>
          </select>
        </label>
        <label class="campo">
          <span>{m.atalhos_rotulo()}</span>
          <input type="text" bind:value={fLabel} maxlength="24" />
        </label>
        <div class="campo">
          <span>{m.atalhos_icone()}</span>
          <div class="glifos" role="radiogroup" aria-label={m.atalhos_icone()}>
            {#each Object.keys(GLYPHS) as g (g)}
              <button type="button" class="glifo" class:sel={!fEmoji.trim() && fGlyph === g}
                      role="radio" aria-checked={!fEmoji.trim() && fGlyph === g} aria-label={g}
                      onclick={() => { fGlyph = g; fEmoji = ''; }}>
                <ShortcutIcon icon={`glifo:${g}`} />
              </button>
            {/each}
            <input class="emoji" type="text" bind:value={fEmoji} maxlength="4"
                   placeholder={m.atalhos_emoji_dica()} aria-label={m.atalhos_emoji_dica()} />
          </div>
        </div>
        <label class="campo">
          <span>{fType === 'shell' ? m.atalhos_comando() : m.atalhos_texto()}</span>
          <input type="text" bind:value={fContent} list={fType === 'send_text' ? 'atalho-skills' : undefined}
                 placeholder={fType === 'shell' ? m.atalhos_comando_dica() : m.atalhos_texto_dica()} />
          {#if fType === 'send_text'}
            <datalist id="atalho-skills">
              {#each suggestions as sk (sk)}<option value={sk}></option>{/each}
            </datalist>
          {/if}
        </label>
        {#if fType === 'send_text'}
          <label class="liga">
            <input type="checkbox" bind:checked={fSendDirect} />
            <span>{m.atalhos_send_direct()}</span>
            <small>{m.atalhos_send_direct_ajuda()}</small>
          </label>
        {/if}
        <label class="liga">
          <input type="checkbox" bind:checked={fConfirm} />
          <span>{m.atalhos_confirm()}</span>
        </label>
        <div class="form-acoes">
          <button class="btn" onclick={() => (formOpen = false)}>{m.comum_cancelar()}</button>
          <button class="btn primario" onclick={submitForm} disabled={!formValid}>{m.comum_confirmar()}</button>
        </div>
      </div>
    {:else}
      <button class="btn" onclick={openNew}>{m.atalhos_add()}</button>
    {/if}

    <div class="rodape">
      <button class="btn" onclick={() => void restoreDefaults()} disabled={saving}
              title={m.atalhos_restaurar_ajuda()}>{m.atalhos_restaurar()}</button>
      <span class="feedback">
        {#if saveError}<span class="erro">{saveError}</span>
        {:else if saved}{m.atalhos_salvo()}{/if}
      </span>
      <button class="btn primario" onclick={() => void save()} disabled={!dirty || saving}>
        {m.atalhos_salvar()}
      </button>
    </div>
  {/if}
</div>

<style>
  /* Container query, não media query: quem aperta a linha é a largura do PAINEL (regra do repo). */
  .at { container-type: inline-size; display: flex; flex-direction: column; gap: var(--space-3); }
  .sub { margin: 0; font-size: var(--text-sm); color: var(--text-secondary); }
  .estado { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .erro { color: var(--danger, #e5484d); }

  .linhas { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
  .linha {
    display: flex; align-items: center; gap: var(--space-3);
    padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--surface-inset);
  }
  .linha.arrastando { opacity: 0.45; }
  .alca { flex-shrink: 0; color: var(--text-muted); cursor: grab; font-size: var(--text-sm); user-select: none; }
  .ico {
    width: 32px; height: 32px; flex-shrink: 0;
    display: inline-flex; align-items: center; justify-content: center;
    border-radius: var(--radius-sm); background: var(--surface-raised);
    color: var(--text-secondary);
  }
  .txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .rotulo { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .detalhe {
    font-size: var(--text-xs); color: var(--text-muted); font-family: var(--font-mono);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .acoes { display: flex; gap: 2px; flex-shrink: 0; }
  .mini {
    min-width: 30px; min-height: 30px; border-radius: var(--radius-sm);
    background: transparent; color: var(--text-secondary); font-size: var(--text-sm);
  }
  .mini:hover { background: var(--surface-raised); color: var(--text-primary); }
  .mini:disabled { opacity: 0.35; }

  .repor { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); font-size: var(--text-xs); color: var(--text-muted); }
  .chip {
    font-size: var(--text-xs); padding: 3px 10px; border-radius: var(--radius-full);
    background: var(--surface-raised); color: var(--text-secondary);
    border: 1px solid var(--border-subtle);
  }
  .chip:hover { color: var(--text-primary); }

  .form {
    display: flex; flex-direction: column; gap: var(--space-3);
    padding: var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-subtle); background: var(--surface-inset);
  }
  .campo { display: flex; flex-direction: column; gap: var(--space-1); font-size: var(--text-sm); color: var(--text-secondary); }
  .campo input[type='text'], .campo select {
    padding: 8px 10px; border-radius: var(--radius-sm);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    color: var(--text-primary); font-size: var(--text-sm);
  }
  .glifos { display: flex; flex-wrap: wrap; gap: 2px; align-items: center; }
  .glifo {
    width: 34px; height: 34px; display: inline-flex; align-items: center; justify-content: center;
    border-radius: var(--radius-sm); background: transparent; color: var(--text-secondary);
  }
  .glifo:hover { background: var(--surface-raised); }
  .glifo.sel { background: var(--accent-dim); color: var(--accent); }
  .emoji { width: 96px; padding: 6px 8px; border-radius: var(--radius-sm);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    color: var(--text-primary); font-size: var(--text-sm); }
  .liga { display: grid; grid-template-columns: auto 1fr; gap: 2px var(--space-2); align-items: center; font-size: var(--text-sm); color: var(--text-primary); }
  .liga small { grid-column: 2; color: var(--text-muted); font-size: var(--text-xs); }
  .form-acoes { display: flex; justify-content: flex-end; gap: var(--space-2); }

  .btn {
    align-self: flex-start;
    padding: 7px 14px; border-radius: var(--radius-md); font-size: var(--text-sm); font-weight: 600;
    background: var(--surface-raised); color: var(--text-primary);
    border: 1px solid var(--border-subtle);
  }
  .btn:hover { background: var(--bg-hover); }
  .btn:disabled { opacity: 0.45; }
  .btn.primario { background: var(--accent); color: #fff; border-color: transparent; }

  .rodape { display: flex; align-items: center; gap: var(--space-2); margin-top: var(--space-2); }
  .feedback { flex: 1; text-align: right; font-size: var(--text-xs); color: var(--success, #30a46c); }
  .feedback .erro { color: var(--danger, #e5484d); }

  @container (max-width: 480px) {
    .acoes { flex-direction: column; }
    .rodape { flex-wrap: wrap; }
  }
</style>

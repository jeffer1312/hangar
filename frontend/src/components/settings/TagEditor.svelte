<script lang="ts">
  // Etiqueta da máquina ou de uma conta, gravada no servidor dela (`/api/tags`). Quem chama dá o
  // título; aqui moram leitura, os quatro estados e a gravação.
  import { untrack } from 'svelte';
  import * as m from '../../paraglide/messages';
  import {
    getTagsForServer, putTagsForServer, tagChangeOf, tagDraftOf, tagHex, ACCOUNT_ICONS, MACHINE_ICONS, TAG_COLORS,
    TAG_COLOR_HEX, TAG_NAME_MAX, type TagColor, type TagDraft, type TagsDoc,
  } from '@hangar/core';
  import type { Server } from '../../lib/auth';
  import TagIcon from '../icons/TagIcon.svelte';

  interface Props {
    server: Server;
    // Ausente = etiqueta da máquina; com id = a da conta (id de `/api/credenciais`).
    account?: { id: string; provider?: string | null } | null;
  }
  let { server, account = null }: Props = $props();

  const ICON_LABEL: Record<string, () => string> = {
    laptop: m.tags_icon_laptop, monitor: m.tags_icon_monitor, server: m.tags_icon_server, house: m.tags_icon_house,
    briefcase: m.tags_icon_briefcase, cloud: m.tags_icon_cloud, 'credit-card': m.tags_icon_credit_card,
    provider: m.tags_icon_provider, 'key-round': m.tags_icon_key_round, wallet: m.tags_icon_wallet,
    'user-round': m.tags_icon_user_round, 'circle-user': m.tags_icon_circle_user, contact: m.tags_icon_contact,
    'badge-check': m.tags_icon_badge_check, 'at-sign': m.tags_icon_at_sign, ticket: m.tags_icon_ticket,
    'id-card': m.tags_icon_id_card,
  };
  const COLOR_LABEL: Record<TagColor, () => string> = {
    slate: m.tags_color_slate, blue: m.tags_color_blue, green: m.tags_color_green, lime: m.tags_color_lime,
    amber: m.tags_color_amber, orange: m.tags_color_orange, pink: m.tags_color_pink, violet: m.tags_color_violet,
  };

  const uid = $props.id();
  const icons = $derived<readonly TagDraft['icon'][]>(account ? ACCOUNT_ICONS : MACHINE_ICONS);

  let load = $state<'loading' | 'error' | 'old' | 'ready'>('loading');
  let name = $state('');
  let icon = $state<TagDraft['icon']>('monitor');
  let color = $state<TagColor | null>(null);
  let saving = $state(false);
  let saved = $state(false);
  let saveError = $state('');

  function apply(doc: TagsDoc) {
    const draft = tagDraftOf(doc, account?.id);
    name = draft.name; icon = draft.icon; color = draft.color;
  }

  // Resposta de uma leitura antiga (troca de máquina ou de conta) não sobrescreve a atual.
  let seq = 0;
  async function read() {
    const mine = ++seq;
    load = 'loading'; saved = false; saveError = '';
    try {
      const doc = await getTagsForServer(server);
      if (mine !== seq) return;
      if (!doc) { load = 'old'; return; }
      apply(doc);
      load = 'ready';
    } catch {
      if (mine === seq) load = 'error';
    }
  }
  // Relê só quando muda o alvo: o pai recria os objetos a cada render.
  const target = $derived(`${server.id}\n${account?.id ?? ''}`);
  $effect(() => { void target; untrack(() => void read()); });

  async function save() {
    if (saving) return;
    saving = true; saved = false; saveError = '';
    try {
      const doc = await putTagsForServer(server, tagChangeOf({ name, icon, color }, account?.id));
      if (!doc) { load = 'old'; return; }
      apply(doc);
      saved = true;
    } catch (e) {
      saveError = (e as { status?: number }).status === 409 ? m.tags_full() : m.tags_save_failed();
    } finally {
      saving = false;
    }
  }
  const edit = (fn: () => void) => { fn(); saved = false; saveError = ''; };
</script>

<div class="te">
  {#if load === 'loading'}
    <p class="te-msg" role="status">{m.comum_carregando()}</p>
  {:else if load === 'error'}
    <p class="te-msg te-err" role="alert">{m.tags_load_failed()}</p>
    <div class="te-actions"><button type="button" class="te-btn" onclick={read}>{m.tags_apply_retry()}</button></div>
  {:else if load === 'old'}
    <p class="te-msg">{m.tags_old_server()}</p>
  {:else}
    {#if !account}
      <label class="te-field">
        <span class="te-label">{m.tags_name()}<small>{m.tags_name_hint()}</small></span>
        <input class="te-input" value={name} maxlength={TAG_NAME_MAX} autocomplete="off" spellcheck={false}
               oninput={(ev) => { const v = ev.currentTarget.value; edit(() => (name = v)); }} />
      </label>
    {/if}
    <span class="te-label" id="{uid}-icon">{m.tags_icon()}</span>
    <div class="te-row" role="group" aria-labelledby="{uid}-icon">
      {#each icons as k (k)}
        <button type="button" class="te-opt" aria-pressed={icon === k} aria-label={ICON_LABEL[k]()} title={ICON_LABEL[k]()}
                style="--te-cor: {tagHex(color) ?? 'currentColor'};"
                onclick={() => edit(() => (icon = k))}>
          <TagIcon icon={k} provider={account?.provider} size={16} />
        </button>
      {/each}
    </div>
    <span class="te-label" id="{uid}-color">{m.tags_color()}</span>
    <div class="te-row" role="group" aria-labelledby="{uid}-color">
      <button type="button" class="te-opt te-auto" aria-pressed={color === null} onclick={() => edit(() => (color = null))}>
        {m.tags_color_auto()}
      </button>
      {#each TAG_COLORS as c (c)}
        <button type="button" class="te-opt te-swatch" aria-pressed={color === c} aria-label={COLOR_LABEL[c]()}
                title={COLOR_LABEL[c]()} style="--te-sw: {TAG_COLOR_HEX[c]};" onclick={() => edit(() => (color = c))}></button>
      {/each}
    </div>
    <div class="te-actions">
      <button type="button" class="te-btn pri" disabled={saving} onclick={save}>{saving ? m.tags_saving() : m.tags_save()}</button>
      {#if saved}<span class="te-msg" role="status">{m.tags_saved()}</span>{/if}
      {#if saveError}<span class="te-msg te-err" role="alert">{saveError}</span>{/if}
    </div>
  {/if}
</div>

<style>
  .te { display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-2) var(--space-3); }
  .te-msg { margin: 0; font-size: 13px; color: var(--text-secondary); }
  .te-err { color: var(--error); }
  .te-field { display: flex; flex-direction: column; gap: 4px; }
  .te-label { font-size: 13px; color: var(--text-primary); display: flex; flex-direction: column; }
  .te-label small { color: var(--text-muted); font-size: 11px; }
  .te-input {
    background: var(--surface-inset); color: var(--text-primary); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm); padding: 8px 10px; font: inherit;
  }
  .te-row { display: flex; flex-wrap: wrap; gap: 6px; }
  .te-opt {
    min-width: 44px; min-height: 44px; display: inline-flex; align-items: center; justify-content: center;
    border-radius: var(--radius-sm); border: 1px solid var(--border-subtle); background: transparent;
    color: var(--te-cor, var(--text-secondary)); cursor: pointer;
  }
  .te-opt[aria-pressed='true'] { border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
  .te-auto { padding: 0 10px; color: var(--text-secondary); font-size: 12px; }
  .te-swatch::after { content: ''; width: 18px; height: 18px; border-radius: 50%; background: var(--te-sw); }
  .te-actions { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); }
  .te-btn {
    min-height: 36px; padding: 0 14px; border-radius: var(--radius-sm); border: 1px solid var(--border-subtle);
    background: transparent; color: var(--text-primary); font: inherit; cursor: pointer;
  }
  .te-btn.pri { background: var(--accent); border-color: var(--accent); color: #fff; }
  .te-btn:disabled { opacity: 0.6; cursor: default; }
</style>

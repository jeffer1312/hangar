import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { ACCOUNT_ICONS, MACHINE_ICONS, TAG_COLORS, TAG_NAME_MAX, getTagsForServer, putTagsForServer, serverColor,
  tagChangeOf, tagDraftOf, tagHex, type AccountIcon, type MachineIcon, type Server, type TagDraft } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { haptic } from '../../ui/haptics';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors } from '../config/colors';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Notice } from '../machines/parts';
import { ACCOUNT_ICON_LABEL, ACCOUNT_ICON_NAME, COLOR_LABEL, MACHINE_ICON_LABEL, MACHINE_ICON_NAME } from './tagVisuals';
import * as m from '../../paraglide/messages';

export type TagTarget =
  | { kind: 'machine'; server: Server }
  | { kind: 'account'; server: Server; accountId: string; provider: string };

type Draft = TagDraft;

const same = (a: Draft | null, b: Draft | null) => !!a && !!b && a.name.trim() === b.name.trim() && a.icon === b.icon && a.color === b.color;

/** Etiqueta da máquina ou de uma conta, gravada no servidor da máquina (`/api/tags`). */
export function TagEditor({ target }: { target: TagTarget }) {
  const c = useSettingsColors();
  const [phase, setPhase] = useState<'loading' | 'old' | 'error' | 'ready'>('loading');
  const [saved, setSaved] = useState<Draft | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [attempt, setAttempt] = useState(0);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const { server } = target;
  const accountId = target.kind === 'account' ? target.accountId : '';
  useEffect(() => {
    let current = true;
    setPhase('loading');
    setError('');
    getTagsForServer(server).then((doc) => {
      if (!current) return;
      if (!doc) { setPhase('old'); return; }
      const next = tagDraftOf(doc, accountId);
      setSaved(next); setDraft(next); setPhase('ready');
    }, () => { if (current) setPhase('error'); });
    return () => { current = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- o alvo muda só por servidor e conta
  }, [server.id, server.baseUrl, server.token, accountId, attempt]);

  if (phase === 'loading') return <View style={styles.center} accessibilityLabel={m.comum_carregando()}><ActivityIndicator color={c.muted} /></View>;
  if (phase === 'old') return <Notice text={m.tags_old_server()} />;
  if (phase === 'error' || !draft) return <>
    <Notice text={m.tags_load_failed()} error />
    <MachineGroup><MachineRow title={m.tags_apply_retry()} action onPress={() => setAttempt((n) => n + 1)} /></MachineGroup>
  </>;

  const pick = (patch: Partial<Draft>) => {
    haptic.selection();
    setMessage(''); setError('');
    setDraft((d) => (d ? { ...d, ...patch } : d));
  };
  const save = async () => {
    if (busy) return;
    const sent = draft;
    setBusy(true); setError(''); setMessage('');
    try {
      const doc = await putTagsForServer(server, tagChangeOf(sent, accountId));
      if (!alive.current) return;
      if (!doc) { haptic.error(); setError(m.tags_old_server()); return; }
      const next = tagDraftOf(doc, accountId);
      setSaved(next); setDraft(next);
      haptic.success(); setMessage(m.tags_saved());
    } catch (e) {
      if (!alive.current) return;
      haptic.error();
      setError((e as { status?: number }).status === 409 ? m.tags_full() : m.tags_save_failed());
    } finally {
      if (alive.current) setBusy(false);
    }
  };

  const tint = tagHex(draft.color) ?? (target.kind === 'machine' ? serverColor(server.id) : c.muted);
  const icons: readonly (MachineIcon | AccountIcon)[] = target.kind === 'machine' ? MACHINE_ICONS : ACCOUNT_ICONS;
  const iconLabel = (icon: MachineIcon | AccountIcon) => target.kind === 'machine'
    ? MACHINE_ICON_LABEL[icon as MachineIcon]() : ACCOUNT_ICON_LABEL[icon as AccountIcon]();
  const glyph = (icon: MachineIcon | AccountIcon, color: string) => icon === 'provider' && target.kind === 'account'
    ? <ProviderGlyph provider={target.provider} size={18} inline color={color} />
    : <Icon name={target.kind === 'machine' ? MACHINE_ICON_NAME[icon as MachineIcon] : ACCOUNT_ICON_NAME[icon as Exclude<AccountIcon, 'provider'>]} size={18} color={color} />;

  return <>
    {target.kind === 'machine' ? <View style={styles.block}>
      <MachineGroup>
        <MachineField label={m.tags_name()} value={draft.name} maxLength={TAG_NAME_MAX} editable={!busy}
          placeholder={server.label} autoCapitalize="words" onChangeText={(name) => { setMessage(''); setDraft({ ...draft, name }); }} />
      </MachineGroup>
      <MachineHelp>{m.tags_name_hint()}</MachineHelp>
    </View> : null}
    <SectionCard icon="Tag" title={m.tags_icon()}>
      <View accessibilityRole="radiogroup">
        {icons.map((icon) => <Choice key={icon} label={iconLabel(icon)} selected={draft.icon === icon} disabled={busy}
          leading={glyph(icon, tint)} onPress={() => pick({ icon })} />)}
      </View>
    </SectionCard>
    <SectionCard icon="Palette" title={m.tags_color()}>
      <View accessibilityRole="radiogroup">
        <Choice label={m.tags_color_auto()} selected={draft.color === null} disabled={busy}
          leading={<View style={[styles.swatch, styles.auto, { borderColor: c.muted }]} />} onPress={() => pick({ color: null })} />
        {TAG_COLORS.map((color) => <Choice key={color} label={COLOR_LABEL[color]()} selected={draft.color === color} disabled={busy}
          leading={<View style={[styles.swatch, { backgroundColor: tagHex(color)! }]} />} onPress={() => pick({ color })} />)}
      </View>
    </SectionCard>
    <MachineGroup>
      <MachineRow title={busy ? m.tags_saving() : m.tags_save()} action disabled={busy || same(draft, saved)} onPress={() => void save()} />
    </MachineGroup>
    <Notice text={error} error /><Notice text={message} />
  </>;
}

function Choice({ label, selected, disabled, leading, onPress }: {
  label: string; selected: boolean; disabled: boolean; leading: ReactNode; onPress: () => void;
}) {
  const c = useSettingsColors();
  return <Pressable accessibilityRole="radio" accessibilityLabel={label} accessibilityState={{ selected, disabled }}
    disabled={disabled} onPress={onPress}
    style={({ pressed }) => [styles.row, { borderTopColor: c.border, opacity: disabled ? 0.5 : 1 }, pressed && { backgroundColor: c.hover }]}>
    <View style={styles.leading} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">{leading}</View>
    <Text style={[styles.label, { color: c.text }]} numberOfLines={1}>{label}</Text>
    {selected ? <Icon name="Check" size={18} color={c.accent} /> : null}
  </Pressable>;
}

const styles = StyleSheet.create((theme) => ({
  center: { minHeight: 88, alignItems: 'center', justifyContent: 'center' },
  block: { gap: theme.base.space[2] },
  row: { minHeight: 48, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[2], borderTopWidth: 1 },
  leading: { width: 24, alignItems: 'center', justifyContent: 'center' },
  label: { flex: 1, minWidth: 0, fontSize: theme.base.text.base },
  swatch: { width: 16, height: 16, borderRadius: theme.base.radius.full },
  auto: { borderWidth: 1.5, borderStyle: 'dashed' },
}));

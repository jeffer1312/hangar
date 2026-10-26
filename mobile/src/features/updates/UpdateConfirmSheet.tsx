import { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { StyleSheet } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { forClient, getAtualizacaoEm, getUpdateNotesEm, type ClientUpdateNotes } from '@hangar/core';
import { useSettingsColors } from '../config/colors';
import { Icon } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { Notice } from '../machines/parts';
import { NewsSections, PrimaryButton, SheetHeader, styles, useNewsTones, useSmallMarkdown } from './WhatsNewSheet';
import { savePendingNews, type ConfirmRequest } from './pending';
import * as m from '../../paraglide/messages';

type Loaded = { notes: ClientUpdateNotes | null; version: string; from: string; failed: boolean };

async function load(request: ConfirmRequest): Promise<Loaded> {
  let version = '';
  let from = '';
  // A rota dos avisos não busca no remoto: a consulta com procurar=1 atualiza o alvo antes.
  try {
    const { versao_legivel: v } = await getAtualizacaoEm(request.server, true);
    from = v?.backend ?? '';
    if (v?.backend && v.remoto) version = `${v.backend} → ${v.remoto}`;
  } catch { /* sem a linha da versão; os avisos ainda podem vir */ }
  try {
    const notes = await getUpdateNotesEm(request.server);
    return { notes: notes ? forClient(notes, 'mobile') : null, version, from, failed: false };
  } catch {
    return { notes: null, version, from, failed: true };
  }
}

export function UpdateConfirmSheet({ request, onClose }: { request: ConfirmRequest | null; onClose: () => void }) {
  const insets = useSafeAreaInsets();
  const c = useSettingsColors();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [understood, setUnderstood] = useState(false);
  const answered = useRef(false);

  useEffect(() => {
    if (!request) return;
    let alive = true;
    void load(request).then((value) => { if (alive) setLoaded(value); });
    // Fechar pelo gesto ou pelo X é cancelar.
    return () => { alive = false; if (!answered.current) request.resolve(false); };
  }, [request]);

  const close = () => { answered.current = true; request?.resolve(false); onClose(); };
  if (!request) return <ScrollView contentContainerStyle={[styles.sheet, { paddingBottom: insets.bottom + 24 }]}>
    <SheetHeader title={m.mobile_machines_update()} onClose={onClose} />
    <Notice error text={m.update_confirm_gone()} />
  </ScrollView>;

  const notes = loaded?.notes;
  const warnings = notes?.warnings ?? [];
  const blocked = !loaded || (warnings.length > 0 && !understood);
  const confirm = () => {
    if (blocked) return;
    answered.current = true;
    savePendingNews(request.server.id, { cards: notes?.cards ?? [], fixes: notes?.fixes ?? [] }, loaded?.from);
    request.resolve(true);
    onClose();
  };
  const all = request.count !== undefined;

  return <ScrollView contentContainerStyle={[styles.sheet, { paddingBottom: insets.bottom + 24 }]}>
    <SheetHeader onClose={close} subtitle={loaded?.version || undefined} title={all
      ? m.update_confirm_all_title({ n: String(request.count) })
      : m.mobile_machines_confirm_operation({ action: m.mobile_machines_update(), name: request.server.label })} />
    <Text style={[styles.body, { color: c.muted }]}>{all ? m.update_confirm_all_body() : m.mobile_machines_confirm_operation_body()}</Text>
    {!loaded ? <View style={[styles.actions, { alignItems: 'center' }]}>
      <ActivityIndicator color={c.muted} />
      <Text style={[styles.body, { color: c.muted }]}>{m.update_notes_loading()}</Text>
    </View> : null}
    {loaded?.failed ? <Notice error text={m.update_notes_failed()} /> : null}
    {warnings.length ? <Warnings warnings={warnings} understood={understood}
      onToggle={() => { haptic.selection(); setUnderstood((v) => !v); }} /> : null}
    {notes ? <NewsSections cards={notes.cards} fixes={notes.fixes} /> : null}
    <View style={styles.actions}>
      <Pressable accessibilityRole="button" onPress={close}
        style={({ pressed }) => [styles.button, { borderWidth: 1, borderColor: c.borderStrong }, pressed && { backgroundColor: c.hover }]}>
        <Text style={[styles.buttonLabel, { color: c.text }]}>{m.comum_cancelar()}</Text>
      </Pressable>
      <PrimaryButton label={m.update_confirm_action()} disabled={blocked} onPress={confirm} />
    </View>
  </ScrollView>;
}

/** Bloco âmbar: o "Entendi" mora dentro dele e libera o Atualizar. */
function Warnings({ warnings, understood, onToggle }: { warnings: ClientUpdateNotes['warnings']; understood: boolean; onToggle: () => void }) {
  const c = useSettingsColors();
  const t = useNewsTones();
  const md = useSmallMarkdown(c.text);
  return <View style={[own.warn, { backgroundColor: t.amberBg, borderColor: t.amberLine }]}>
    {warnings.map((w) => <View key={w.id} style={own.warnItem}>
      <Text accessibilityRole="header" style={[own.warnTitle, { color: t.amber }]}>{`${m.update_notes_important()}: ${w.title}`}</Text>
      <EnrichedMarkdownText markdown={w.text} markdownStyle={md} flavor="github" />
    </View>)}
    <Pressable accessibilityRole="checkbox" accessibilityLabel={m.update_notes_understood()} accessibilityState={{ checked: understood }}
      onPress={onToggle} style={own.check}>
      <View style={[own.box, understood ? { backgroundColor: t.violet, borderColor: t.violet } : { borderColor: c.borderStrong }]}>
        {understood ? <Icon name="Check" size={14} color={t.onViolet} /> : null}
      </View>
      <Text style={[own.checkLabel, { color: c.text }]}>{m.update_notes_understood()}</Text>
    </Pressable>
  </View>;
}

const own = StyleSheet.create((theme) => ({
  warn: { borderWidth: 1, borderRadius: theme.base.radius.md, borderCurve: 'continuous', paddingHorizontal: theme.base.space[3], paddingTop: theme.base.space[3], gap: theme.base.space[2] },
  warnItem: { gap: theme.base.space[1] },
  warnTitle: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  check: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  box: { width: 22, height: 22, borderRadius: theme.base.radius.xs, borderWidth: 1.5, alignItems: 'center', justifyContent: 'center' },
  checkLabel: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
}));

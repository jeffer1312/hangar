import { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { StyleSheet } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { forClient, getAtualizacaoEm, getUpdateNotesEm, type ClientUpdateNotes, type Server, type UpdateMode, type UpdateWarning } from '@hangar/core';
import { useSettingsColors } from '../config/colors';
import { Icon } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { Notice } from '../machines/parts';
import { NewsSections, PrimaryButton, SheetHeader, styles, useNewsTones, useSmallMarkdown } from './WhatsNewSheet';
import { savePendingNews, type ConfirmRequest, type UpdateAcceptances } from './pending';
import * as m from '../../paraglide/messages';

// `title` só no "Atualizar todas": os avisos vêm agrupados por máquina.
type WarningGroup = { server: Server; title?: string; warnings: UpdateWarning[] };
// `runnable`: quantas máquinas vão rodar; no "Atualizar todas", só as que leram os avisos.
type Loaded = { notes: ClientUpdateNotes | null; version: string; from: string; failed: boolean; groups: WarningGroup[]; unreachable: Server[]; runnable: number };

const loadFailed = (request: ConfirmRequest): Loaded =>
  ({ notes: null, version: '', from: '', failed: true, groups: [], unreachable: [], runnable: request.machines ? 0 : 1 });

async function load(request: ConfirmRequest): Promise<Loaded> {
  const version = await loadVersion(request.server);
  if (!request.machines) {
    try {
      const raw = await getUpdateNotesEm(request.server);
      const notes = raw ? forClient(raw, 'mobile') : null;
      return { ...version, notes, failed: false, unreachable: [], runnable: 1,
        groups: notes?.warnings.length ? [{ server: request.server, warnings: notes.warnings }] : [] };
    } catch {
      return { ...loadFailed(request), ...version };
    }
  }
  const { run, failed } = await request.machines;
  // A versão nova é a mesma em todas: as novidades vêm da ativa, ou de outra que respondeu.
  const source = run.find((entry) => entry.server.id === request.server.id && entry.notes) ?? run.find((entry) => entry.notes);
  return { ...version, notes: source?.notes ? forClient(source.notes, 'mobile') : null, failed: false, unreachable: failed, runnable: run.length,
    groups: run.filter((entry) => entry.notes?.warnings.length).map((entry) => ({
      server: entry.server, title: m.update_all_warnings_from({ server: entry.server.label }), warnings: entry.notes!.warnings })) };
}

async function loadVersion(server: Server): Promise<{ version: string; from: string }> {
  // A rota dos avisos não busca no remoto: a consulta com procurar=1 atualiza o alvo antes.
  try {
    const { versao_legivel: v } = await getAtualizacaoEm(server, true);
    return { from: v?.backend ?? '', version: v?.backend && v.remoto ? `${v.backend} → ${v.remoto}` : '' };
  } catch {
    return { version: '', from: '' }; // sem a linha da versão; os avisos ainda podem vir
  }
}

export function UpdateConfirmSheet({ request, onClose }: { request: ConfirmRequest | null; onClose: () => void }) {
  const insets = useSafeAreaInsets();
  const c = useSettingsColors();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [understood, setUnderstood] = useState(false);
  const [modes, setModes] = useState<Record<string, UpdateMode>>({});
  const answered = useRef(false);

  useEffect(() => {
    if (!request) return;
    let alive = true;
    void load(request).then((value) => { if (alive) setLoaded(value); }, () => { if (alive) setLoaded(loadFailed(request)); });
    // Fechar pelo gesto ou pelo X é cancelar.
    return () => { alive = false; if (!answered.current) request.resolve(false); };
  }, [request]);

  const close = () => { answered.current = true; request?.resolve(false); onClose(); };
  if (!request) return <ScrollView contentContainerStyle={[styles.sheet, { paddingBottom: insets.bottom + 24 }]}>
    <SheetHeader title={m.mobile_machines_update()} onClose={onClose} />
    <Notice error text={m.update_confirm_gone()} />
  </ScrollView>;

  const notes = loaded?.notes;
  const groups = loaded?.groups ?? [];
  const blocked = !loaded || loaded.runnable === 0 || (groups.length > 0 && !understood);
  const confirm = () => {
    if (blocked) return;
    answered.current = true;
    savePendingNews(request.server.id, { cards: notes?.cards ?? [], fixes: notes?.fixes ?? [] }, loaded?.from);
    const accept: UpdateAcceptances = Object.fromEntries(groups.map(({ server, warnings }) => [server.id, {
      ids: warnings.map((w) => w.id),
      ...(warnings.some((w) => w.choice === 'mode') ? { mode: modes[server.id] ?? 'package' } : {}),
    }]));
    request.resolve(true, accept);
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
    {loaded?.unreachable.map((server) => <Notice key={server.id} error text={m.update_all_warnings_failed({ server: server.label })} />)}
    {groups.length ? <Warnings groups={groups} understood={understood} modes={modes}
      onMode={(id, mode) => setModes((v) => ({ ...v, [id]: mode }))}
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
function Warnings({ groups, understood, modes, onMode, onToggle }: { groups: WarningGroup[]; understood: boolean;
  modes: Record<string, UpdateMode>; onMode: (serverId: string, mode: UpdateMode) => void; onToggle: () => void }) {
  const c = useSettingsColors();
  const t = useNewsTones();
  const md = useSmallMarkdown(c.text);
  return <View style={[own.warn, { backgroundColor: t.amberBg, borderColor: t.amberLine }]}>
    {groups.map((g) => <View key={g.server.id} style={own.warnItem}>
      {g.title ? <Text accessibilityRole="header" style={[own.warnTitle, { color: c.text }]}>{g.title}</Text> : null}
      {g.warnings.map((w) => <View key={w.id} style={own.warnItem}>
        <Text accessibilityRole="header" style={[own.warnTitle, { color: t.amber }]}>{`${m.update_notes_important()}: ${w.title}`}</Text>
        <EnrichedMarkdownText markdown={w.text} markdownStyle={md} flavor="github" />
        {w.choice === 'mode' ? <ModeChoice value={modes[g.server.id] ?? 'package'} onChange={(mode) => onMode(g.server.id, mode)}
          label={g.title ? `${g.title}: ${m.update_choice_title()}` : m.update_choice_title()} /> : null}
      </View>)}
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

/** Como a máquina roda o Hangar, pedido pelo aviso da migração; o pacote é a recomendação. */
function ModeChoice({ value, onChange, label: groupLabel }: { value: UpdateMode; onChange: (mode: UpdateMode) => void; label: string }) {
  const c = useSettingsColors();
  const t = useNewsTones();
  const options = [
    ['package', m.update_choice_package(), m.update_choice_package_desc()],
    ['developer', m.update_choice_developer(), m.update_choice_developer_desc()],
  ] as const;
  return <View accessibilityRole="radiogroup" accessibilityLabel={groupLabel}>
    <Text style={[own.checkLabel, { color: c.text }]}>{m.update_choice_title()}</Text>
    {options.map(([mode, label, desc]) => {
      const on = value === mode;
      return <Pressable key={mode} accessibilityRole="radio" accessibilityLabel={label} accessibilityHint={desc} accessibilityState={{ checked: on }}
        onPress={() => { if (on) return; haptic.selection(); onChange(mode); }} style={own.option}>
        <View style={[own.radio, { borderColor: on ? t.violet : c.borderStrong }]}>
          {on ? <View style={[own.dot, { backgroundColor: t.violet }]} /> : null}
        </View>
        <View style={own.optionText}>
          <Text style={[own.checkLabel, { color: c.text }]}>{label}</Text>
          <Text style={[own.optionDesc, { color: c.muted }]}>{desc}</Text>
        </View>
      </Pressable>;
    })}
  </View>;
}

const own = StyleSheet.create((theme) => ({
  warn: { borderWidth: 1, borderRadius: theme.base.radius.md, borderCurve: 'continuous', paddingHorizontal: theme.base.space[3], paddingTop: theme.base.space[3], gap: theme.base.space[2] },
  warnItem: { gap: theme.base.space[1] },
  warnTitle: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  check: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  box: { width: 22, height: 22, borderRadius: theme.base.radius.xs, borderWidth: 1.5, alignItems: 'center', justifyContent: 'center' },
  checkLabel: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  option: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingVertical: theme.base.space[2] },
  radio: { width: 22, height: 22, borderRadius: 11, borderWidth: 1.5, alignItems: 'center', justifyContent: 'center' },
  dot: { width: 12, height: 12, borderRadius: 6 },
  optionText: { flex: 1, gap: 2 },
  optionDesc: { fontSize: theme.base.text.xs },
}));

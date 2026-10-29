import { memo, useState } from 'react';
import { Alert, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { cwdParts, providerName, rotuloEstado, type AggSession } from '@hangar/core';
import { SessionAvatar } from '../sessions/SessionAvatar';
import { HeadlessSignal } from '../sessions/HeadlessSignal';
import { Icon } from '../../ui/Icon';
import { Sheet } from '../../ui/Sheet';
import { MACHINE_ICON_NAME } from '../tags/tagVisuals';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

type Props = {
  session: AggSession;
  members?: AggSession[];
  label?: string | null;
  full?: boolean;
  selected?: { serverId: string; name: string };
  onPick(session: AggSession): void;
};

const keyOf = (s: { serverId: string; name: string }) => `${s.serverId}::${s.name}`;
const waits = (s: AggSession) => s.state === 'awaiting_input' || !!s.pending_questions || !!s.limited;
const description = (s: AggSession) => `${s.serverLabel}, ${rotuloEstado(s.state)}${s.limited ? `, ${m.sessao_limite()}` : ''}`;
const agentOf = (s: AggSession) => s.provider === 'claude' && /gpt|openai|codex/i.test(s.engine ?? '') ? 'codex' : s.provider;

// O prefixo compartilhado sai só até um separador; o nome inteiro continua no toque longo.
function shortName(name: string, names: string[]): string {
  const cut = Math.max(0, ...names.filter(other => other !== name).map(other => {
    let common = 0;
    while (common < name.length && name[common]?.toLowerCase() === other[common]?.toLowerCase()) common++;
    const prefix = name.slice(0, common);
    return Math.max(prefix.lastIndexOf('-'), prefix.lastIndexOf('_')) + 1;
  }));
  const rest = Array.from(name.slice(cut) || name);
  return rest.length > 12 ? `${rest.slice(0, 11).join('')}…` : rest.join('');
}

export const SessionChip = memo(function SessionChip({ session, members = [], label, full = false, selected, onPick }: Props) {
  const { theme } = useUnistyles();
  const [open, setOpen] = useState(false);
  const children = [...members].sort((a, b) => Number(waits(b)) - Number(waits(a)));
  const selectedKey = selected && keyOf(selected);
  const on = [session, ...members].some(s => keyOf(s) === selectedKey);
  const grouped = members.length > 0;
  const pendingQuestions = (session.pending_questions ?? 0) + children.filter(waits).length;
  const names = [session, ...members].map(s => s.name);
  const agents = new Map<AggSession['provider'], number>();
  for (const child of children) {
    const agent = agentOf(child);
    agents.set(agent, (agents.get(agent) ?? 0) + 1);
  }
  const counts = [...agents].map(([provider, count]) => `${providerName(provider)}: ${count}`).join(', ');
  const groupTitle = label?.trim() || session.name;
  const folder = cwdParts(session.git_cwd || session.cwd).base;
  const showFolder = full && folder && folder !== session.name;
  const branch = full && session.branch && !['main', 'master'].includes(session.branch) ? session.branch : null;
  const ahead = full ? session.git_ahead ?? 0 : 0;
  const behind = full ? session.git_behind ?? 0 : 0;
  const meta = [showFolder ? folder : null, branch, ahead > 0 || behind > 0 ? m.git_sync_titulo({ ahead, behind }) : null].filter(Boolean).join(', ');
  const aria = `${session.name}, ${description(session)}${label ? `, ${label.trim()}` : ''}${meta ? `, ${meta}` : ''}${pendingQuestions > 0 ? `, ${m.ask_perguntas()}: ${pendingQuestions}` : ''}${full && session.headless ? `, ${m.criar_modo_exec_headless()}` : ''}`;
  const pick = (s: AggSession) => { setOpen(false); onPick(s); };
  const showName = (s: AggSession) => Alert.alert(s.name, description(s));
  return (
    <View testID="session-tab" style={styles.slot}>
      <View style={[styles.tab, full && styles.fullTab, {
        backgroundColor: on || grouped ? superficie(theme, 0.8) : 'transparent',
        borderColor: on ? theme.tokens.accent.base : children.some(waits) ? theme.tokens.status.warning : theme.tokens.border.default,
      }]}>
        <View style={styles.body}>
          <Pressable onPress={() => pick(session)} onLongPress={() => showName(session)} accessibilityRole="button"
            accessibilityState={{ selected: on }} accessibilityLabel={aria}
            style={({ pressed }) => [styles.main, { backgroundColor: pressed ? superficie(theme, 0.8) : 'transparent' }]}>
            <SessionAvatar session={session} size={22} />
            <View style={styles.text}>
              <View style={styles.title}>
                <Text numberOfLines={1} style={[styles.name, styles.titleName]}>{session.name}</Text>
                {full && session.headless ? <HeadlessSignal /> : null}
              </View>
              {full ? <View style={styles.meta}>
                {showFolder ? <View style={styles.metaItem}><Icon name="Folder" size={11} /><Text numberOfLines={1} style={styles.metaText}>{folder}</Text></View> : null}
                {branch ? <View style={styles.metaItem}><Icon name="GitBranch" size={11} /><Text numberOfLines={1} style={styles.metaText}>{branch}</Text></View> : null}
                {ahead > 0 ? <Text style={[styles.metaText, styles.sync, { color: theme.tokens.accent.base }]}>{`↑${ahead}`}</Text> : null}
                {behind > 0 ? <Text style={[styles.metaText, styles.sync, { color: theme.tokens.status.warning }]}>{`↓${behind}`}</Text> : null}
              </View> : null}
            </View>
            {pendingQuestions > 0 ? <Text style={styles.pending}>{`? ${pendingQuestions}`}</Text> : null}
          </Pressable>
          {grouped ? <Pressable testID="agent-count" onPress={() => setOpen(true)} accessibilityRole="button"
            accessibilityLabel={`${m.tab_group_children({ n: members.length })}, ${counts}`} accessibilityState={{ expanded: open }} style={styles.counter}>
            <View style={styles.agents}>{[...agents].map(([provider, count]) => <View key={provider ?? 'hangar'} style={styles.agent}>
              <SessionAvatar session={{ provider, state: null }} size={16} />
              <Text style={styles.count}>{count}</Text>
            </View>)}</View>
            <Icon name="ChevronDown" size={12} />
          </Pressable> : null}
        </View>
        <View testID="machine-legend" pointerEvents="none" style={styles.machine}>
          {/* Nome e cor já vêm da etiqueta da máquina pelo core; o ícone também. */}
          <Icon name={session.machineTag ? MACHINE_ICON_NAME[session.machineTag.icon] : 'Monitor'} size={11} color={session.serverColor} />
          <Text numberOfLines={1} style={styles.metaText}>{session.serverLabel}</Text>
        </View>
      </View>
      {grouped ? <View style={styles.children}>
          {children.slice(0, 3).map(child => <Pressable key={keyOf(child)} testID="child-chip" accessibilityRole="button"
            accessibilityLabel={`${child.name}, ${description(child)}`} accessibilityState={{ selected: keyOf(child) === selectedKey }}
            onPress={() => pick(child)} onLongPress={() => showName(child)} style={styles.childTarget}>
            <View style={[styles.child, { borderColor: keyOf(child) === selectedKey ? theme.tokens.accent.base : theme.tokens.border.default }]}>
              <SessionAvatar session={child} size={10} />
              <Text numberOfLines={1} style={styles.childName}>{shortName(child.name, names)}</Text>
            </View>
          </Pressable>)}
          {children.length > 3 ? <Pressable onPress={() => setOpen(true)} style={styles.more} accessibilityRole="button"
            accessibilityLabel={m.tab_group_children({ n: members.length })}>
            <Text style={styles.count}>{`+${children.length - 3}`}</Text>
          </Pressable> : null}
      </View> : null}
      {open ? <Sheet open onDismiss={() => setOpen(false)} sizes={['medium', 'large']} scrollable>
        <ScrollView contentContainerStyle={styles.sheet}>
          <View style={styles.sheetHeader}>
            <Text style={styles.sheetTitle}>{groupTitle}</Text>
            <Pressable onPress={() => setOpen(false)} accessibilityRole="button" accessibilityLabel={m.git_fechar()} style={styles.more}>
              <Icon name="X" size={20} />
            </Pressable>
          </View>
          {[session, ...children].map(s => <Pressable key={keyOf(s)} onPress={() => pick(s)} onLongPress={() => showName(s)}
            accessibilityRole="button" accessibilityLabel={`${s.name}, ${description(s)}`} accessibilityState={{ selected: keyOf(s) === selectedKey }} style={styles.member}>
            <SessionAvatar session={s} size={30} />
            <View style={styles.text}><Text style={styles.name}>{s.name}</Text><Text style={styles.metaText}>{description(s)}</Text></View>
          </Pressable>)}
        </ScrollView>
      </Sheet> : null}
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  slot: { position: 'relative', width: 240, flexShrink: 0, paddingTop: theme.base.space[2], paddingBottom: theme.base.space[2] + 14 },
  tab: { position: 'relative', height: 44, borderRadius: theme.base.radius.md, borderWidth: 1 },
  fullTab: { height: 52 },
  body: { flex: 1, flexDirection: 'row', alignItems: 'center', paddingHorizontal: theme.base.space[1] },
  main: { flex: 1, minWidth: 0, minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], borderRadius: theme.base.radius.md },
  text: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  title: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  titleName: { flexShrink: 1 },
  pending: { flexShrink: 0, fontSize: theme.base.text.xxs, color: theme.tokens.status.warning },
  name: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  machine: { position: 'absolute', top: -8, right: 8, maxWidth: 160, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], paddingHorizontal: theme.base.space[1], borderRadius: theme.base.radius.sm, backgroundColor: theme.tokens.bg.base },
  meta: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], overflow: 'hidden' },
  metaItem: { flexDirection: 'row', alignItems: 'center', flexShrink: 1, minWidth: 0, gap: theme.base.space[0.5] },
  metaText: { fontSize: theme.base.text.xxxs, color: theme.tokens.text.muted, flexShrink: 1 },
  sync: { flexShrink: 0 },
  counter: { minWidth: 44, minHeight: 44, maxWidth: 96, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[0.5] },
  agents: { flexDirection: 'row', flexWrap: 'wrap', flexShrink: 1 },
  agent: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[0.5] },
  count: { fontSize: theme.base.text.xxxs, color: theme.tokens.accent.base },
  children: { position: 'absolute', bottom: 0, left: 6, right: 6, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  childTarget: { width: 56, minHeight: 44, justifyContent: 'center' },
  child: { minHeight: 16, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[0.5], borderRadius: theme.base.radius.full, borderWidth: 1, paddingHorizontal: theme.base.space[0.5], backgroundColor: theme.tokens.bg.base },
  childName: { flexShrink: 1, fontSize: theme.base.text.xxxs, color: theme.tokens.text.secondary },
  more: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  sheet: { padding: theme.base.space[4], gap: theme.base.space[2] },
  sheetHeader: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  sheetTitle: { flex: 1, fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  member: { flexDirection: 'row', alignItems: 'center', minHeight: 44, paddingVertical: theme.base.space[2], gap: theme.base.space[2] },
}));

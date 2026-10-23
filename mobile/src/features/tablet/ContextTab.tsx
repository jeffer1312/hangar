import { useMemo } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { groupGlobal, parseStatusLine, providerName, rotuloEstado, type AggSession } from '@hangar/core';
import { chatStore } from '../../stores/chat';
import { useSessions } from '../../stores/sessions';
import { useServers } from '../../stores/servers';
import { useIdentities } from '../pair/identities';
import { composerStatus, usageWindows } from '../../chat/usage';
import { ActionsGrid } from './ActionsGrid';
import { isWorking } from './barRows';
import * as m from '../../paraglide/messages';

const clamp = (pct: number) => Math.min(100, Math.max(0, pct));

export function ContextTab({ serverId, name, onPick }: { serverId: string; name: string; onPick?: (session: AggSession) => void }) {
  const { theme } = useUnistyles();
  const statusLine = chatStore(serverId, name).use((s) => s.statusLine);
  const loading = chatStore(serverId, name).use((s) => s.stateEvent === null);
  const session = useSessions((s) => s.byServerRecord?.[serverId]?.find((item) => item.name === name)
    ?? s.rows.find((item) => item.serverId === serverId && item.name === name) ?? null);
  const fields = useMemo(() => parseStatusLine(statusLine, session), [statusLine, session]);
  const status = composerStatus(fields, session);
  const windows = usageWindows(fields);
  const pct = status.ctxPct;
  const tone = (value: number) => value >= 90 ? theme.tokens.status.error
    : value >= 70 ? theme.tokens.status.warning : theme.tokens.accent.base;
  const bar = (label: string, value: number) => (
    <View accessibilityRole="progressbar" accessibilityLabel={label}
      accessibilityValue={{ min: 0, max: 100, now: Math.round(clamp(value)) }} style={styles.bar}>
      <View style={[styles.fill, { width: `${clamp(value)}%`, backgroundColor: tone(value) }]} />
    </View>
  );

  return (
    <ScrollView style={styles.root} contentContainerStyle={styles.content}>
      {session && (session.pair_gid || session.pair_peers?.length) ? <GroupCard session={session} onPick={onPick} /> : null}
      <View style={styles.section}>
        <Text style={styles.label} accessibilityRole="header">{m.ctx_contexto()}</Text>
        {pct != null ? <>
          <Text style={[styles.percent, { color: tone(pct) }]}>{Math.round(clamp(pct))}%</Text>
          {bar(m.ctx_uso_contexto(), pct)}
        </> : loading ? <ActivityIndicator color={theme.tokens.text.muted} accessibilityLabel={m.comum_carregando()} />
          : <Text style={styles.muted}>{m.tablet_context_empty()}</Text>}
      </View>

      {status.cost ? <View style={styles.row}>
        <Text style={styles.label}>{m.uso_custo()}</Text>
        <Text style={styles.value}>{status.cost}</Text>
      </View> : null}

      {windows.length ? <View style={styles.section}>
        <Text style={styles.label} accessibilityRole="header">{m.uso_secao_cota()}</Text>
        {windows.map((window) => <View key={window.key} style={styles.section}>
          <View style={styles.row}>
            <Text style={styles.label}>{window.label}</Text>
            <Text style={styles.value}>{Math.round(clamp(window.pct))}%</Text>
          </View>
          {bar(window.label, window.pct)}
          {window.reset ? <Text style={styles.muted}>{m.uso_reset({ quando: window.reset })}</Text> : null}
        </View>)}
      </View> : null}

      {status.folder || status.branch ? <View style={styles.section}>
        <Text style={styles.label} accessibilityRole="header">{m.uso_linha_projeto()}</Text>
        {status.folder ? <Text style={styles.value} selectable>{status.folder}</Text> : null}
        {status.branch ? <Text style={styles.muted} selectable>{status.branch}</Text> : null}
      </View> : null}

      <View style={styles.section}>
        <Text style={styles.label} accessibilityRole="header">{m.tablet_actions()}</Text>
        <ActionsGrid serverId={serverId} name={name} />
      </View>
    </ScrollView>
  );
}

const GROUP_VISIBLE_ROWS = 4;

function groupRank(session: AggSession): number {
  if (session.state === 'awaiting_input') return 0;
  if (isWorking(session)) return 1;
  return session.state === 'idle' ? 2 : 3;
}

function GroupCard({ session, onPick }: { session: AggSession; onPick?: (session: AggSession) => void }) {
  const { serverId, name } = session;
  const rows = useSessions(s => s.rows);
  const servers = useServers(s => s.servers);
  const crossMachine = !!session.pair_owner || !!session.pair_peers?.some(p => p.includes('::'));
  const identityServers = useMemo(() => crossMachine ? servers : [], [crossMachine, servers]);
  const { identities } = useIdentities(identityServers);
  const group = useMemo(() => {
    const found = groupGlobal(rows, identities).find(g => g.members.some(member =>
      member.kind === 'here' && member.server === serverId && member.name === name));
    if (!found || found.members.length < 2) return null;
    const members = found.members.flatMap(member => member.kind === 'here'
      ? rows.find(s => s.serverId === member.server && s.name === member.name) ?? [] : []);
    members.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.serverId < b.serverId ? -1 : a.serverId > b.serverId ? 1 : 0);
    const others = members.filter(s => s.serverId !== serverId || s.name !== name);
    // Com rolagem, quem pede resposta e quem trabalha vem primeiro; o nome desempata (sort estável).
    if (others.length > GROUP_VISIBLE_ROWS) others.sort((a, b) => groupRank(a) - groupRank(b));
    const away = found.members.flatMap(member => member.kind === 'missing' ? [`${member.server_id}::${member.name}`]
      : member.kind === 'outside' ? [`${member.alias}::${member.session}`] : []);
    return { total: found.members.length, members, others, away, working: others.filter(isWorking).length,
      label: members.map(s => s.pair_task?.trim()).find(Boolean) };
  }, [rows, identities, serverId, name]);
  const relation = (member: AggSession) => {
    if (!group) return '';
    const parent = group.members.find(s => s.serverId === member.serverId && s.name === member.linked_by);
    if (parent) return parent.serverId === serverId && parent.name === name ? m.native_group_panel_child_of_here()
      : m.native_group_panel_child_of({ name: parent.name });
    const children = group.members.filter(s => s.serverId === member.serverId && s.linked_by === member.name);
    if (!children.length) return '';
    return children.length === 1 && children[0].serverId === serverId && children[0].name === name
      ? m.native_group_panel_parent_of_here() : m.native_group_panel_parent_of({ names: children.map(s => s.name).join(', ') });
  };
  if (!group) return null;
  return (
    <View style={styles.group}>
      <Text style={styles.label} accessibilityRole="header">{m.native_group_panel_title({ count: group.total })}</Text>
      {group.label ? <Text style={styles.muted}>{group.label}</Text> : null}
      {group.working ? <Text style={styles.muted}>{m.native_group_panel_working({ count: group.working })}</Text> : null}
      {group.others.map(member => {
        const relationship = relation(member);
        const content = <>
          <Text style={styles.value}>{member.name}</Text>
          <Text style={styles.muted}>{[member.serverLabel, providerName(member.provider), member.engine || member.model].filter(Boolean).join(' · ')}</Text>
          {relationship ? <Text style={styles.relation}>{relationship}</Text> : null}
          {member.last_reply ? <Text style={styles.muted} numberOfLines={2}>{member.last_reply}</Text> : null}
        </>;
        const key = `${member.serverId}::${member.name}`;
        return onPick ? <Pressable key={key} accessibilityRole="button"
          accessibilityLabel={`${m.linked_children_open()}: ${member.name}, ${member.serverLabel}, ${rotuloEstado(member.state)}`}
          onPress={() => onPick(member)} style={styles.member}>{content}</Pressable>
          : <View key={key} style={styles.member}>{content}</View>;
      })}
      {group.away.map(address => <Text key={address} style={styles.muted}>{address}</Text>)}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1 },
  content: { padding: theme.base.space[4], gap: theme.base.space[5] },
  section: { gap: theme.base.space[2] },
  group: { gap: theme.base.space[2], padding: theme.base.space[3], borderWidth: 1,
    borderColor: theme.tokens.border.subtle, borderRadius: theme.base.radius.md },
  member: { minHeight: 44, gap: theme.base.space[1], paddingVertical: theme.base.space[2] },
  relation: { fontSize: theme.base.text.xs, color: theme.tokens.accent.base },
  row: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: theme.base.space[3] },
  label: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.secondary },
  percent: { fontSize: theme.base.text.xl * 2, fontWeight: theme.base.weight.bold },
  value: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium, color: theme.tokens.text.primary, flexShrink: 1 },
  muted: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  bar: { height: 6, borderRadius: theme.base.radius.full, overflow: 'hidden', backgroundColor: theme.tokens.border.subtle },
  fill: { height: '100%', borderRadius: theme.base.radius.full },
}));

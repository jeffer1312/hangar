import { useEffect, useMemo } from 'react';
import { ActivityIndicator, FlatList, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useKeyboardState } from 'react-native-keyboard-controller';
import { machineNoticeVisible, rotuloEstado, type AggSession } from '@hangar/core';
import { useTabletPrefs, type SessionsBarPlacement } from '../../stores/tabletPrefs';
import { useSessions } from '../../stores/sessions';
import { useServers } from '../../stores/servers';
import { useIdentities } from '../pair/identities';
import { SessionRow } from '../sessions/SessionRow';
import { SessionAvatar } from '../sessions/SessionAvatar';
import { useNoticeClock } from '../sessions/useNoticeClock';
import { Glass } from '../../ui/Glass';
import { Icon } from '../../ui/Icon';
import { StateDot } from '../../ui/StateDot';
import { superficie } from '../../theme/superficie';
import { SessionChip } from './SessionChip';
import { barRows, type BarItem } from './barRows';
import * as m from '../../paraglide/messages';

type Props = { placement: SessionsBarPlacement; selected?: { serverId: string; name: string }; onPick(s: AggSession): void; onNew(): void };

const keyOf = (s: AggSession) => `${s.serverId}::${s.name}`;

export function SessionsBar({ placement, selected, onPick, onNew }: Props) {
  const { theme } = useUnistyles();
  const rows = useSessions(s => s.rows);
  const loading = useSessions(s => s.loading);
  const byServer = useSessions(s => s.byServer);
  const noticeNow = useNoticeClock(byServer);
  const servers = useServers(s => s.servers);
  const crossMachine = rows.some(s => s.pair_owner || s.pair_peers?.some(p => p.includes('::')));
  const identityServers = useMemo(() => crossMachine ? servers : [], [crossMachine, servers]);
  const { identities } = useIdentities(identityServers);
  const horizontal = placement === 'bottom' || placement === 'top';
  const tabsWorkingFirst = useTabletPrefs(s => s.tabsWorkingFirst);
  const tabsRecentFirst = useTabletPrefs(s => s.tabsRecentFirst);
  const items = useMemo(() => barRows(rows, identities, horizontal, { tabsWorkingFirst, tabsRecentFirst }),
    [rows, identities, horizontal, tabsWorkingFirst, tabsRecentFirst]);
  const tabsDetail = useTabletPrefs(s => s.tabsDetail);
  const collapsed = useTabletPrefs(s => s.collapsed);
  const toggleCollapsed = useTabletPrefs(s => s.toggleCollapsed);
  const keyboardVisible = useKeyboardState(s => s.isVisible);
  useEffect(() => useSessions.getState().retain(), []);

  if ((horizontal && collapsed) || (placement === 'bottom' && keyboardVisible)) return null;
  const rail = placement === 'left' && collapsed;
  const isSelected = (s: AggSession) => s.serverId === selected?.serverId && s.name === selected?.name;
  const sessionItem = (s: AggSession) => {
    if (placement === 'left' && !rail) return <SessionRow key={keyOf(s)} session={s} mostrarServidor compact selected={isSelected(s)} onPress={onPick} />;
    if (rail) return (
      <Pressable key={keyOf(s)} onPress={() => onPick(s)} accessibilityRole="button"
        accessibilityLabel={`${s.name}, ${s.serverLabel}, ${rotuloEstado(s.state)}`} accessibilityState={{ selected: isSelected(s) }}
        style={({ pressed }) => [styles.railItem, { backgroundColor: isSelected(s) || pressed ? superficie(theme, 0.8) : 'transparent' }]}>
        <SessionAvatar session={s} size={30} />
        <View style={styles.dot}><StateDot state={s.state} size={6} /></View>
      </Pressable>
    );
    return <SessionChip key={keyOf(s)} session={s} selected={selected} full={tabsDetail === 'full'} onPick={onPick} />;
  };
  const renderItem = ({ item }: { item: BarItem }) => item.kind === 'single' ? sessionItem(item.session) : horizontal ? (
    <SessionChip session={item.members[0]} members={item.members.slice(1)} label={item.label}
      selected={selected} full={tabsDetail === 'full'} onPick={onPick} />
  ) : (
    <View style={styles.group}>
      {!rail && item.label ? <Text numberOfLines={1}
        style={[styles.groupLabel, { color: theme.tokens.text.muted }]}>{item.label}</Text> : null}
      {item.members.map(sessionItem)}
    </View>
  );

  return (
    <Glass variant="panel" style={[styles.bar, horizontal ? [styles.horizontal, tabsDetail === 'full' && styles.fullHorizontal] : rail ? styles.rail : styles.left]}>
      <FlatList key={horizontal ? 'horizontal' : 'vertical'} data={items} horizontal={horizontal}
        style={styles.list} contentContainerStyle={[styles.items, horizontal && styles.horizontalItems]}
        keyExtractor={item => item.kind === 'single' ? keyOf(item.session) : item.key}
        renderItem={renderItem} showsHorizontalScrollIndicator={false} showsVerticalScrollIndicator={false}
        ListEmptyComponent={<View style={styles.emptyState}>
          {loading ? <ActivityIndicator accessibilityLabel={m.lista_carregando()} /> :
            <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.tablet_sessions_empty()}</Text>}
          {byServer.filter(b => machineNoticeVisible(b, noticeNow)).map(b =><Text key={b.server.id} accessibilityRole="alert"
            style={[styles.empty, { color: theme.tokens.status.error }]}>
            {b.error === 'offline' ? m.lista_servidor_offline({ label: b.server.label }) : `${b.server.label}: ${b.error}`}
          </Text>)}
        </View>} />
      <View style={[styles.controls, horizontal && styles.horizontalControls]}>
        <Pressable onPress={toggleCollapsed} style={styles.control} accessibilityRole="button"
          accessibilityLabel={collapsed ? m.tablet_bar_expand() : m.tablet_bar_collapse()} accessibilityState={{ expanded: !collapsed }}>
          <Icon name="PanelLeft" size={20} />
        </Pressable>
        <Pressable onPress={onNew} style={styles.control} accessibilityRole="button" accessibilityLabel={m.tablet_new_session()}>
          <Icon name="Plus" size={22} color={theme.tokens.accent.base} />
        </Pressable>
      </View>
    </Glass>
  );
}

const styles = StyleSheet.create((theme) => ({
  bar: { flexShrink: 0 },
  left: { width: 250, flexDirection: 'column' },
  rail: { width: 64, flexDirection: 'column' },
  horizontal: { height: 74, flexDirection: 'row', alignItems: 'center' },
  fullHorizontal: { height: 82 },
  list: { flex: 1 },
  items: { padding: theme.base.space[1], gap: theme.base.space[1], flexGrow: 1 },
  horizontalItems: { paddingVertical: 0, alignItems: 'center' },
  railItem: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center', borderRadius: theme.base.radius.md },
  dot: { position: 'absolute', right: 5, bottom: 5 },
  group: { borderWidth: 1, borderColor: theme.tokens.border.subtle, borderRadius: theme.base.radius.md, overflow: 'hidden' },
  groupLabel: { fontSize: theme.base.text.xxxs, paddingHorizontal: theme.base.space[3], paddingTop: theme.base.space[1] },
  empty: { fontSize: theme.base.text.xxxs, padding: theme.base.space[1], alignSelf: 'center', flexShrink: 1 },
  emptyState: { justifyContent: 'center', flexShrink: 1 },
  controls: { alignItems: 'center', padding: theme.base.space[1] },
  horizontalControls: { flexDirection: 'row' },
  control: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
}));

import type { ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { usePathname, useRouter } from 'expo-router';
import type { AggSession } from '@hangar/core';
import { useTabletPrefs } from '../../stores/tabletPrefs';
import { useServers } from '../../stores/servers';
import { Screen } from '../../ui/Screen';
import { HangarMark } from '../../ui/HangarMark';
import { Icon } from '../../ui/Icon';
import { LiveVoiceChatButton } from '../voice/LiveVoiceChatButton';
import { SessionsBar } from './SessionsBar';
import { ContextPanel } from './ContextPanel';
import * as m from '../../paraglide/messages';

type Props = { selected?: { serverId: string; name: string }; children: ReactNode; active?: boolean };

// A área central conserva o mesmo lugar na árvore ao girar ou mudar a posição da barra.
export function TabletShell({ selected, children, active = true }: Props) {
  const router = useRouter();
  const pathname = usePathname();
  const { theme } = useUnistyles();
  const placement = useTabletPrefs(s => s.placement);
  const collapsed = useTabletPrefs(s => s.collapsed);
  const toggleCollapsed = useTabletPrefs(s => s.toggleCollapsed);
  const panelOpen = useTabletPrefs(s => s.panelOpen);
  const activeId = useServers(s => s.activeId);
  const voiceServer = selected?.serverId ?? activeId;
  const pickSession = (s: AggSession) => {
    const path = `/s/${encodeURIComponent(s.serverId)}/${encodeURIComponent(s.name)}` as never;
    if (selected && /^\/s\/[^/]+\/[^/]+\/?$/.test(pathname)) router.replace(path);
    else if (selected) router.dismissTo(path);
    else router.push(path);
  };
  const bar = (position: typeof placement) => active && placement === position && (position === 'left' || !collapsed) ? (
    <SessionsBar placement={placement} selected={selected}
      onPick={pickSession}
      onNew={() => router.push('/create')} />
  ) : null;

  return (
    <Screen edges={active ? undefined : []} background={active}>
      {active ? <View style={styles.top}>
        {collapsed && placement !== 'left' ? <Pressable onPress={toggleCollapsed} accessibilityRole="button"
          accessibilityLabel={m.tablet_bar_expand()} accessibilityState={{ expanded: false }} style={styles.button}>
          <Icon name="PanelLeft" size={20} color={theme.tokens.text.secondary} />
        </Pressable> : null}
        <Pressable onPress={() => router.dismissTo('/')} accessibilityRole="button" accessibilityLabel={m.chat_ir_inicio()} style={styles.brand}>
          <HangarMark size={24} color={theme.tokens.accent.base} />
          <Text style={styles.brandName}>{m.native_brand()}</Text>
        </Pressable>
        <View style={styles.spacer} />
        <Pressable onPress={() => router.push('/search')} accessibilityRole="button" accessibilityLabel={m.lista_buscar()} style={styles.search}>
          <Icon name="Search" size={20} color={theme.tokens.text.secondary} />
          <Text style={styles.searchLabel}>{m.lista_buscar()}</Text>
        </Pressable>
        <View style={styles.spacer} />
        {voiceServer ? <LiveVoiceChatButton serverId={voiceServer} name={selected?.name} /> : null}
        <Pressable onPress={() => router.push('/costs')} accessibilityRole="button" accessibilityLabel={m.nav_custos()} style={styles.button}>
          <Icon name="ChartNoAxesCombined" size={20} color={theme.tokens.text.secondary} />
        </Pressable>
        <Pressable onPress={() => router.push('/reviews' as never)} accessibilityRole="button" accessibilityLabel={m.native_review_title()} style={styles.button}>
          <Icon name="GitPullRequest" size={20} color={theme.tokens.text.secondary} />
        </Pressable>
        <Pressable onPress={() => router.push('/config')} accessibilityRole="button" accessibilityLabel={m.config_modal_titulo()} style={styles.button}>
          <Icon name="Settings" size={20} color={theme.tokens.text.secondary} />
        </Pressable>
      </View> : null}
      <View style={active && placement === 'top' ? styles.horizontal : undefined}>{bar('top')}</View>
      <View style={[styles.row, active && styles.tabletRow]}>
        {bar('left')}
        <View key="conversation" style={styles.center}>{children}</View>
        {active && selected && panelOpen ? <ContextPanel serverId={selected.serverId} name={selected.name}
          focused={/^\/s\/[^/]+\/[^/]+\/?$/.test(pathname)} onPick={pickSession} /> : null}
      </View>
      <View style={active && placement === 'bottom' ? styles.horizontal : undefined}>{bar('bottom')}</View>
    </Screen>
  );
}

const styles = StyleSheet.create(theme => ({
  top: { minHeight: 44, flexDirection: 'row', alignItems: 'center', paddingHorizontal: theme.base.space[3], gap: theme.base.space[2] },
  brand: { minHeight: 44, minWidth: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  brandName: { color: theme.tokens.text.primary, fontSize: theme.base.text.base, fontWeight: theme.base.weight.bold },
  button: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  spacer: { flex: 1 },
  search: { minHeight: 44, maxWidth: 340, flex: 1, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  searchLabel: { color: theme.tokens.text.secondary, fontSize: theme.base.text.sm, flexShrink: 1 },
  row: { flex: 1, flexDirection: 'row', minHeight: 0 },
  tabletRow: { gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], paddingBottom: theme.base.space[2] },
  center: { flex: 1, minWidth: 0, minHeight: 0 },
  horizontal: { marginHorizontal: theme.base.space[2] },
}));

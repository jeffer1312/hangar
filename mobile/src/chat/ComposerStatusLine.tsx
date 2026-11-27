import { useMemo, useState, type ReactNode } from 'react';
import { Pressable, Text, View, useWindowDimensions } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { parseStatusLine } from '@hangar/core';
import { chatStore } from '../stores/chat';
import { useSessions } from '../stores/sessions';
import { permissionLabel } from '../features/pills/permissionLabel';
import { Icon } from '../ui/Icon';
import { ContextRing } from './ContextRing';
import { UsageSheet } from './UsageSheet';
import { composerStatus, fitComposerItems } from './usage';
import { sessionAccountLabel } from './sessionAccountLabel';
import { useAgyUsageWindows } from './useAgyUsageWindows';
import * as m from '../paraglide/messages';

interface Props {
  serverId: string;
  name: string;
  onOpenSettings?: (which: 'mode' | 'account' | 'usage') => void;
}

// O chat usa uma folha só; consumidores sem controlador conservam a folha de uso própria.
export function ComposerStatusLine({ serverId, name, onOpenSettings }: Props) {
  const { theme } = useUnistyles();
  const { fontScale } = useWindowDimensions();
  const chat = chatStore(serverId, name);
  const statusLine = chat.use((s) => s.statusLine);
  const stateEvent = chat.use((s) => s.stateEvent);
  const session = useSessions((s) => s.byServerRecord?.[serverId]?.find((x) => x.name === name)
    ?? s.rows.find((x) => x.serverId === serverId && x.name === name) ?? null);
  const status = useMemo(() => composerStatus(parseStatusLine(statusLine, session), session), [statusLine, session]);
  const accountLabel = session ? sessionAccountLabel(session) : null;
  const agyWindows = useAgyUsageWindows(serverId, session, stateEvent?.state === 'idle');
  const [usageOpen, setUsageOpen] = useState(false);
  const [availableWidth, setAvailableWidth] = useState(0);
  const [measured, setMeasured] = useState<Record<string, number>>({});

  const tone = (pct: number) => (pct >= 90 ? theme.tokens.status.error : pct >= 70 ? theme.tokens.status.warning : theme.tokens.text.muted);
  const muted = theme.tokens.text.muted;
  const openUsage = () => onOpenSettings ? onOpenSettings('usage') : setUsageOpen(true);
  const items: { key: string; node: ReactNode; spoken: string; shrink?: boolean; onPress?: () => void }[] = [];
  if (status.folder) {
    items.push({ key: 'folder', spoken: status.folder, shrink: true, onPress: openUsage, node: (
      <>
        <Icon name="Folder" size={12} color={muted} />
        <Text style={[styles.text, styles.shrinkText, { color: muted }]} numberOfLines={1} ellipsizeMode="tail">{status.folder}</Text>
      </>
    ) });
  }
  if (status.branch) {
    items.push({ key: 'branch', spoken: status.branch, shrink: true, onPress: openUsage, node: (
      <>
        <Icon name="GitBranch" size={12} color={muted} />
        <Text style={[styles.text, styles.shrinkText, { color: muted }]} numberOfLines={1} ellipsizeMode="tail">{status.branch}</Text>
      </>
    ) });
  }
  if (status.added || status.removed) {
    const diff = [status.added ? `+${status.added}` : '', status.removed ? `−${status.removed}` : ''].filter(Boolean).join(' ');
    items.push({ key: 'diff', spoken: diff, onPress: openUsage, node: (
      <Text style={styles.text} numberOfLines={1} ellipsizeMode="tail">
        {status.added ? <Text style={{ color: theme.tokens.status.success }} numberOfLines={1} ellipsizeMode="tail">+{status.added}</Text> : null}
        {status.added && status.removed ? ' ' : ''}
        {status.removed ? <Text style={{ color: theme.tokens.status.error }} numberOfLines={1} ellipsizeMode="tail">−{status.removed}</Text> : null}
      </Text>
    ) });
  }
  if (status.time) {
    items.push({ key: 'time', spoken: status.time, onPress: openUsage, node: (
      <>
        <View style={[styles.dot, { backgroundColor: theme.tokens.status.success }]} />
        <Text style={[styles.text, { color: muted }]} numberOfLines={1} ellipsizeMode="tail">{status.time}</Text>
      </>
    ) });
  }
  const claudeMode = stateEvent?.claude_permission_mode;
  const codexMode = stateEvent?.codex_mode;
  // O Antigravity publica o modo no campo do Claude, com os mesmos nomes.
  const mode = session?.provider === 'claude' || session?.provider === 'antigravity' ? (claudeMode ? permissionLabel(claudeMode) : m.chat_mode_label())
    : session?.provider === 'codex' ? (codexMode === 'plan' ? m.chat_mode_plan() : codexMode === 'default' ? m.chat_mode_normal() : m.chat_mode_label()) : null;
  if (mode) {
    items.push({ key: 'mode', spoken: mode, shrink: true, onPress: onOpenSettings ? () => onOpenSettings('mode') : undefined,
      node: <Text style={[styles.text, styles.shrinkText, { color: muted }]} numberOfLines={1} ellipsizeMode="tail">{mode}</Text> });
  }
  if (accountLabel) {
    items.push({ key: 'account', spoken: accountLabel, shrink: true, onPress: onOpenSettings ? () => onOpenSettings('account') : undefined,
      node: <Text style={[styles.text, styles.shrinkText, { color: muted }]} numberOfLines={1} ellipsizeMode="tail">{accountLabel}</Text> });
  }
  if (status.ctxPct != null) {
    const pct = Math.round(status.ctxPct);
    items.push({ key: 'ctx', spoken: `${m.ctx_contexto()} ${pct}%`, onPress: openUsage, node: (
      <>
        <ContextRing pct={status.ctxPct} size={12} showValue={false} />
        <Text style={[styles.text, { color: tone(status.ctxPct) }]} numberOfLines={1} ellipsizeMode="tail">{pct}%</Text>
      </>
    ) });
  }
  for (const q of status.quotas ?? agyWindows) {
    items.push({ key: q.key, spoken: `${q.label} ${Math.round(q.pct)}%`, onPress: openUsage, node: (
      <Text style={[styles.text, { color: tone(q.pct) }]} numberOfLines={1} ellipsizeMode="tail">{q.key} {Math.round(q.pct)}%</Text>
    ) });
  }
  if (status.cost) {
    items.push({ key: 'cost', spoken: status.cost, onPress: openUsage,
      node: <Text style={[styles.text, { color: muted }]} numberOfLines={1} ellipsizeMode="tail">{status.cost}</Text> });
  }
  if (!items.length) {
    items.push({ key: 'usage', spoken: m.uso_aria(), onPress: openUsage, node: (
      <>
        <Icon name="Gauge" size={12} color={muted} />
        <Text style={[styles.text, { color: muted }]} numberOfLines={1} ellipsizeMode="tail">{m.uso_titulo()}</Text>
      </>
    ) });
  }
  const measurementId = (item: typeof items[number]) => `${fontScale}:${item.spoken}`;
  const widths = fitComposerItems(items.filter(item => measured[item.key] != null)
    .map(item => ({ ...item, width: measured[item.key] })), availableWidth);

  return (
    <View style={styles.line}>
      <View testID="composer-status-items" style={styles.items} onLayout={event => {
        const width = event.nativeEvent.layout.width;
        setAvailableWidth(previous => previous === width ? previous : width);
      }}>
        {items.filter(item => widths[item.key] != null).map(item => {
          const props = { testID: `composer-status-${item.key}`, accessibilityLabel: item.spoken,
            style: [styles.item, { width: widths[item.key] }] };
          const content = <View style={styles.content} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">{item.node}</View>;
          return item.onPress ? (
            <Pressable key={item.key} {...props} accessibilityRole="button" onPress={item.onPress}
              style={({ pressed }) => [...props.style, pressed && styles.pressed]}>{content}</Pressable>
          ) : <View key={item.key} {...props} accessible>{content}</View>;
        })}
      </View>
      {/* Mesma tipografia, sem largura imposta: onLayout lê o tamanho preferido, fora do fluxo. */}
      <View testID="composer-status-measuring" style={styles.measuring} pointerEvents="none"
        accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
        {items.map(item => {
          const id = measurementId(item);
          return <View key={`${item.key}:${id}`} testID={`composer-status-measure-${item.key}`} style={styles.measureItem} onLayout={event => {
            const width = Math.ceil(event.nativeEvent.layout.width);
            setMeasured(previous => previous[item.key] === width
              ? previous : { ...previous, [item.key]: width });
          }}>{item.node}</View>;
        })}
      </View>
      {usageOpen ? <UsageSheet open onClose={() => setUsageOpen(false)} serverId={serverId} name={name} /> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  line: {
    position: 'relative',
    paddingHorizontal: theme.base.space[4],
  },
  pressed: {
    opacity: 0.6,
  },
  items: {
    flexDirection: 'row',
    flexWrap: 'nowrap',
    alignItems: 'center',
    gap: theme.base.space[1],
    minWidth: 0,
    minHeight: 44,
  },
  item: {
    minWidth: 44,
    minHeight: 44,
    flexShrink: 0,
    justifyContent: 'center',
  },
  content: {
    flexDirection: 'row',
    flexWrap: 'nowrap',
    alignItems: 'center',
    minWidth: 0,
    gap: theme.base.space[1],
  },
  measuring: {
    position: 'absolute',
    left: 0,
    top: 0,
    flexDirection: 'row',
    opacity: 0,
  },
  measureItem: {
    flexDirection: 'row',
    flexShrink: 0,
    alignItems: 'center',
    minWidth: 44,
    gap: theme.base.space[1],
  },
  shrinkText: {
    minWidth: 0,
    flexShrink: 1,
  },
  text: {
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xxs,
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: theme.base.radius.full,
  },
}));

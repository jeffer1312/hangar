import { useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Linking, Pressable, ScrollView, Text, View, useWindowDimensions } from 'react-native';
import type { TextStyle, ViewStyle } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { SvgXml } from 'react-native-svg';
import { useKeyboardState } from 'react-native-keyboard-controller';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import * as Clipboard from 'expo-clipboard';
import {
  buttonControl, inputControl, decodeRaster, inkColor, safeHref, textOf,
  activePaneId, followLocalTab, tabFollowsServer, isEmptyBand, BAND_SITE,
  pressPluginButton, showPluginPane, closePluginPane, inputPluginField, pluginFailureText, isMissingRoute,
  type PluginNode as Node, type PluginControl, type PluginInputKind,
} from '@hangar/core';
import { mkMarkdownStyle } from './AssistantBubble';
import { PluginInput } from './PluginInput';
import { LinkedChildren } from './LinkedChildren';
import { Icon } from '../ui/Icon';
import { chatStore } from '../stores/chat';
import { useServers } from '../stores/servers';
import { useMods } from '../stores/mods';
import { toast } from '../ui/Toast';
import * as m from '../paraglide/messages';

export interface PluginTreeProps {
  node: Node;
  place?: number | null;
  onPress?: (button: PluginControl) => void;
  disabled?: boolean;
  inText?: boolean;
  shrinkRow?: boolean;
  onInput?: (field: PluginControl, kind: PluginInputKind, value: string) => Promise<unknown>;
}

const str = (v: unknown) => typeof v === 'string' ? v : '';
const finite = (v: unknown) => typeof v === 'number' && Number.isFinite(v) ? v : undefined;
const percent = (v: unknown): `${number}%` | undefined => typeof v === 'string' && /^\d+(\.\d+)?%$/.test(v) ? v as `${number}%` : undefined;

async function openLink(value: unknown) {
  const href = safeHref(value);
  if (!href) { toast.erro(m.plugin_link_inseguro()); return; }
  try { await Linking.openURL(href); }
  catch { toast.erro(m.plugin_clique_falhou()); }
}

export function PluginTree({ node, place, onPress, onInput, disabled, inText = false, shrinkRow = false }: PluginTreeProps) {
  const { theme } = useUnistyles();
  const markdownStyle = useMemo(() => mkMarkdownStyle(theme), [theme]);
  if (typeof node === 'string' || typeof node === 'number') return inText ? node : <Text style={[styles.text, shrinkRow && styles.truncated]} numberOfLines={shrinkRow ? 1 : undefined} ellipsizeMode="tail">{node}</Text>;
  if (!node || typeof node !== 'object') return null;
  const p = node.props ?? {};
  if (p.display === 'none') return null;
  const shrink = finite(p.flexShrink);
  const childShrinkRow = node.type === 'Box'
    ? !str(p.flexDirection).startsWith('column') && p.position !== 'absolute' && (shrink === undefined ? shrinkRow : shrink > 0)
    : shrinkRow;
  const kids = (node.children ?? []).map((child, index) => {
    const key = child && typeof child === 'object' && typeof child.props?.key === 'string'
      ? JSON.stringify([child.type, child.press?.plugin, child.props.key]) : index;
    return <PluginTree key={key} node={child} place={place} onPress={onPress} onInput={onInput} disabled={disabled} inText={inText || node.type === 'Text'} shrinkRow={childShrinkRow} />;
  });
  const cell = theme.base.text.sm * 0.6;
  const line = theme.base.text.sm * 1.4;
  const color = inkColor(p.color);
  const background = inkColor(p.backgroundColor);
  const textStyle: TextStyle = {
    color: p.inverse === true ? background ?? theme.tokens.bg.base : color ?? (inText ? undefined : theme.tokens.text.primary),
    backgroundColor: p.inverse === true ? color ?? theme.tokens.text.primary : background ?? undefined,
    fontWeight: p.bold === true ? '700' : undefined,
    fontStyle: p.italic === true ? 'italic' : undefined,
    textDecorationLine: p.underline === true && p.strikethrough === true ? 'underline line-through' : p.underline === true ? 'underline' : p.strikethrough === true ? 'line-through' : undefined,
    opacity: p.dimColor === true ? 0.6 : undefined,
  };
  switch (node.type) {
    case 'Text': {
      const wrap = str(p.wrap);
      const truncate = !inText && (wrap.startsWith('truncate') || wrap === 'end' || wrap === 'middle' || shrinkRow && !wrap);
      return <Text style={[!inText && styles.text, textStyle, truncate && styles.truncated]} numberOfLines={truncate ? 1 : undefined}
        ellipsizeMode={wrap.endsWith('start') ? 'head' : wrap.endsWith('middle') || wrap === 'middle' ? 'middle' : 'tail'}>{kids}</Text>;
    }
    case 'Box': {
      const numeric = (...keys: string[]) => keys.map(key => finite(p[key])).find(v => v !== undefined);
      const h = (...keys: string[]) => { const n = numeric(...keys); return n === undefined ? undefined : n * cell; };
      const v = (...keys: string[]) => { const n = numeric(...keys); return n === undefined ? undefined : n * line; };
      const width = finite(p.width);
      const box: ViewStyle = {
        position: p.position === 'absolute' ? 'absolute' : 'relative',
        flexDirection: ['row', 'column', 'row-reverse', 'column-reverse'].includes(str(p.flexDirection)) ? p.flexDirection as ViewStyle['flexDirection'] : 'row',
        flexWrap: ['wrap', 'nowrap', 'wrap-reverse'].includes(str(p.flexWrap)) ? p.flexWrap as ViewStyle['flexWrap'] : undefined,
        flexGrow: finite(p.flexGrow), flexShrink: finite(p.flexShrink) ?? 1,
        alignItems: ['flex-start', 'flex-end', 'center', 'stretch', 'baseline'].includes(str(p.alignItems)) ? p.alignItems as ViewStyle['alignItems'] : undefined,
        alignSelf: ['auto', 'flex-start', 'flex-end', 'center', 'stretch', 'baseline'].includes(str(p.alignSelf)) ? p.alignSelf as ViewStyle['alignSelf'] : undefined,
        justifyContent: ['flex-start', 'flex-end', 'center', 'space-between', 'space-around', 'space-evenly'].includes(str(p.justifyContent)) ? p.justifyContent as ViewStyle['justifyContent'] : undefined,
        width: width !== undefined ? '100%' : percent(p.width),
        maxWidth: width !== undefined && !(place && width >= place) ? width * cell : '100%',
        minWidth: 0,
        columnGap: h('columnGap', 'gap'), rowGap: v('rowGap', 'gap'),
        paddingTop: v('paddingTop', 'paddingY', 'padding'), paddingBottom: v('paddingBottom', 'paddingY', 'padding'),
        paddingLeft: h('paddingLeft', 'paddingX', 'padding'), paddingRight: h('paddingRight', 'paddingX', 'padding'),
        marginTop: v('marginTop', 'marginY', 'margin'), marginBottom: v('marginBottom', 'marginY', 'margin'),
        marginLeft: h('marginLeft', 'marginX', 'margin'), marginRight: h('marginRight', 'marginX', 'margin'),
        backgroundColor: background ?? (p.position === 'absolute' ? theme.tokens.bg.surface : undefined),
        borderWidth: p.borderStyle ? 1 : undefined, borderColor: inkColor(p.borderColor) ?? theme.tokens.border.subtle,
        borderRadius: p.borderStyle === 'round' ? theme.base.radius.sm : undefined,
        overflow: p.overflow === 'hidden' ? 'hidden' : undefined,
        ...(p.position === 'absolute' ? { top: v('top'), bottom: v('bottom'), left: h('left'), right: h('right'), zIndex: 1 } : {}),
      };
      return <View style={box}>{kids}</View>;
    }
    case 'Button': {
      const control = buttonControl(node);
      const label = str(p.label) || textOf(node.children);
      const body = <Text numberOfLines={1} ellipsizeMode="tail" style={[styles.text, textStyle, shrinkRow && styles.truncated, p.variant === 'primary' && !color && { color: theme.tokens.accent.base }]}>{label}</Text>;
      return onPress && control
        ? <Pressable accessibilityRole="button" accessibilityLabel={label} disabled={disabled} accessibilityState={{ disabled: !!disabled }} hitSlop={12} onPress={() => onPress(control)} style={[styles.button, shrinkRow && styles.truncated, p.plain === true && styles.plain, disabled && { opacity: 0.5 }]}>{body}</Pressable>
        : <View style={[styles.button, shrinkRow && styles.truncated, p.plain === true && styles.plain]}>{body}</View>;
    }
    case 'Link': {
      const label = str(p.label) || textOf(node.children) || str(p.href);
      return <Text style={[styles.text, styles.link]} accessibilityRole={safeHref(p.href) ? 'link' : undefined} onPress={safeHref(p.href) ? () => void openLink(p.href) : undefined}>{label}</Text>;
    }
    case 'Markdown': return <EnrichedMarkdownText markdown={str(p.text)} markdownStyle={markdownStyle} flavor="github" onLinkPress={({ url }) => void openLink(url)} />;
    case 'Code': return <Text selectable style={[styles.text, styles.code]}>{str(p.source)}</Text>;
    case 'Image': return <Text style={styles.muted}>{str(p.alt)}</Text>;
    case 'Svg': {
      const width = finite(p.width);
      const height = finite(p.height) ?? line * 2;
      const sizing: ViewStyle = width !== undefined && width > 0 && height > 0 ? { aspectRatio: width / height } : { height };
      return <View style={[styles.svg, { width: '100%', maxWidth: width ?? '100%' }, sizing]} accessibilityRole="image" accessibilityLabel={str(p.alt)}>
        <SvgXml xml={str(p.source)} width="100%" height="100%" onError={() => toast.erro(m.plugin_svg_falhou())} />
      </View>;
    }
    case 'Raster': return <View style={styles.raster}>{decodeRaster(str(p.cells), Number(p.columns) || 0, Number(p.rows) || 0).map((row, i) => (
      <Text key={i} style={styles.code} adjustsFontSizeToFit numberOfLines={1}>{row.map((c, j) => <Text key={j} style={{ color: c.fg ?? theme.tokens.text.primary, backgroundColor: c.bg ?? undefined }}>{c.ch}</Text>)}</Text>
    ))}</View>;
    case 'Input': {
      const control = inputControl(node);
      return <PluginInput key={control ? `${control.plugin}:${control.key}` : ''} frame={node} onInput={onInput && control ? (kind, value) => onInput(control, kind, value) : undefined} />;
    }
    default: return <View style={styles.fallback}>{kids}</View>;
  }
}

export function PluginUi({ serverId, name, readOnly = false }: { serverId: string; name: string; readOnly?: boolean }) {
  const { theme } = useUnistyles();
  const { height } = useWindowDimensions();
  const keyboardHeight = useKeyboardState(state => state.height);
  const availableHeight = Math.max(0, height - keyboardHeight);
  const chat = chatStore(serverId, name);
  const surface = chat.use(s => s.pluginUi);
  const streamError = chat.use(s => s.pluginUiError);
  const visible = useMods(s => s.visible);
  const server = useServers(s => s.servers.find(x => x.id === serverId));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const flight = useRef(false);
  const mounted = useRef(true);
  const previous = useRef<string[]>([]);
  const [local, setLocal] = useState<string | null>(null);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    const ids = surface?.panes.map(pane => pane.id) ?? [];
    setLocal(current => tabFollowsServer(ids, surface?.shownId) ? surface!.shownId! : followLocalTab(previous.current, ids, current));
    previous.current = ids;
  }, [surface]);

  async function action(run: () => Promise<unknown>, generic: () => string) {
    if (flight.current || readOnly || streamError) return;
    if (!server) { setError(m.chat_servidor_removido()); return; }
    flight.current = true;
    setBusy(true);
    setError(null);
    try { await run(); }
    catch (err) { if (mounted.current) setError(pluginFailureText(err, generic)); }
    finally {
      flight.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  const press = (site: string, control: PluginControl) => void action(async () => {
    const reply = await pressPluginButton(name, site, control, server);
    if (!mounted.current) return;
    if (typeof reply.copied === 'string') {
      await Clipboard.setStringAsync(reply.copied);
      toast.ok(m.plugin_copiado());
    }
    if (reply.opened !== undefined) await openLink(reply.opened);
  }, m.plugin_clique_falhou);
  const ids = surface?.panes.map(pane => pane.id) ?? [];
  const active = activePaneId(ids, readOnly && local ? undefined : surface?.shownId, local);
  const pane = surface?.panes.find(pane => pane.id === active);
  const show = (site: string) => {
    if (flight.current || streamError) return;
    if (readOnly || !tabFollowsServer(ids, surface?.shownId)) setLocal(site);
    if (readOnly) return;
    void action(async () => {
      try { await showPluginPane(name, site, server); }
      catch (err) { if (!isMissingRoute(err)) throw err; }
    }, m.plugin_aba_falhou);
  };
  const tree = (node: Node, site: string, place: number | null) => <PluginTree key={site} node={node} place={place}
    disabled={busy || !!streamError} onPress={!readOnly && !streamError ? control => press(site, control) : undefined}
    onInput={surface?.source === 'surface' && !readOnly && !streamError && server
      ? (control, kind, value) => inputPluginField(name, site, control, kind, value, server) : undefined} />;
  return <>
    {visible && surface ? <View style={[styles.wrap, { maxHeight: availableHeight * 0.4 }]}>
      {pane ? <View style={styles.pane} accessibilityLabel={m.plugin_painel_label({ titulo: pane.title })}>
        <View style={styles.header}>
          <ScrollView horizontal style={styles.tabs} contentContainerStyle={styles.tabRow} accessibilityLabel={m.plugin_abas_label()}>
            {surface.panes.map(tab => <Pressable key={tab.id} accessibilityRole="tab" accessibilityLabel={tab.title}
              accessibilityState={{ selected: tab.id === active, disabled: busy || !!streamError }} disabled={busy || !!streamError} onPress={() => show(tab.id)} style={styles.tab}>
              <Text style={[styles.text, tab.id === active ? { color: theme.tokens.accent.base } : styles.muted]}>{tab.title}</Text>
            </Pressable>)}
          </ScrollView>
          {!readOnly ? <Pressable accessibilityRole="button" accessibilityLabel={m.plugin_painel_fechar()} disabled={busy || !!streamError}
            onPress={() => void action(() => closePluginPane(name, pane.id, server), m.plugin_fechar_falhou)} style={styles.close}>
            <Icon name="X" size={18} color={theme.tokens.text.secondary} />
          </Pressable> : null}
        </View>
        <ScrollView nestedScrollEnabled keyboardShouldPersistTaps="handled" style={[styles.verticalScroll, { maxHeight: availableHeight * 0.3 }]} contentContainerStyle={styles.paneBody}>
          {/* Sem desenho o corpo fica vazio, como no nativo: o painel só de estado (o `/btw` sem terminal) nunca traz árvore. */}
          {!pane.tree || typeof pane.tree === 'object' && pane.tree.type === 'engine'
            ? null
            : tree(pane.tree, pane.id, pane.columns)}
        </ScrollView>
      </View> : null}
      {!isEmptyBand(surface.above) ? <ScrollView nestedScrollEnabled keyboardShouldPersistTaps="handled" style={[styles.verticalScroll, { maxHeight: availableHeight * 0.15 }]} contentContainerStyle={styles.band}
        accessibilityLabel={m.plugin_band_label()}>{tree(surface.above, BAND_SITE, surface.columns)}</ScrollView> : null}
      {busy ? <ActivityIndicator size="small" /> : null}
      {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}
    </View> : null}
    <LinkedChildren serverId={serverId} name={name} />
    {streamError ? <Text accessibilityRole="alert" style={styles.error}>{streamError}</Text> : null}
  </>;
}

const styles = StyleSheet.create(theme => ({
  wrap: { flexShrink: 1, overflow: 'hidden', gap: theme.base.space[1], paddingHorizontal: theme.base.space[3] },
  pane: { flexShrink: 1, borderColor: theme.tokens.border.subtle, borderWidth: 1, borderRadius: theme.base.radius.md, overflow: 'hidden' },
  header: { flexDirection: 'row', alignItems: 'center', borderBottomColor: theme.tokens.border.subtle, borderBottomWidth: 1 },
  tabs: { flex: 1 },
  tabRow: { alignItems: 'center' },
  tab: { minHeight: 44, paddingHorizontal: theme.base.space[2], justifyContent: 'center' },
  close: { minHeight: 44, minWidth: 44, alignItems: 'center', justifyContent: 'center' },
  verticalScroll: { flexGrow: 0, flexShrink: 1 },
  paneBody: { padding: theme.base.space[2] },
  band: { padding: theme.base.space[2] },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.xs, padding: theme.base.space[2] },
  text: { color: theme.tokens.text.primary, fontSize: theme.base.text.sm, lineHeight: theme.base.text.sm * 1.4, includeFontPadding: false, flexShrink: 0 },
  truncated: { flexShrink: 1, minWidth: 0 },
  svg: { flexShrink: 1, maxWidth: '100%', alignSelf: 'center', overflow: 'hidden' },
  muted: { color: theme.tokens.text.muted, fontSize: theme.base.text.xs },
  code: { fontFamily: theme.base.fontMono, color: theme.tokens.text.secondary, fontSize: theme.base.text.xs },
  button: { alignSelf: 'flex-start', flexShrink: 0, maxWidth: '100%', justifyContent: 'center', paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm, backgroundColor: 'transparent' },
  plain: { paddingHorizontal: 0 },
  link: { color: theme.tokens.accent.base, textDecorationLine: 'underline', minHeight: 44 },
  raster: { flexShrink: 1, maxWidth: '100%' },
  fallback: { flexDirection: 'row', flexWrap: 'wrap', minWidth: 0 },
}));

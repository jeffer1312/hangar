import { useEffect, type ReactNode } from 'react';
import { Pressable, Switch, Text, TextInput, View, type TextInputProps } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { fmtWhen, intlLocale, liveClaudeSessions, type InstalledPlugin, type Server } from '@hangar/core';
import { InfoNotice } from '../config/InfoNotice';
import { useSettingsColors } from '../config/colors';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { Notice } from '../machines/parts';
import { Icon, type IconName } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';
import { useSessions } from '../../stores/sessions';
import { usePlugins } from './store';
import * as m from '../../paraglide/messages';

const EMPTY: never[] = [];

/** Quantas sessões Claude abertas o servidor tem: só reabrindo pegam a mudança. Número, para só redesenhar quando ele muda. */
export function useLiveClaudeCount(server: Server | null): number {
  useEffect(() => useSessions.getState().retain(), []);
  return useSessions((s) => (server ? liveClaudeSessions(s.byServerRecord[server.id] ?? EMPTY).length : 0));
}

/** Nomes lidos na hora do toque, sem assinar a lista inteira. */
function liveClaudeNames(server: Server): string[] {
  return liveClaudeSessions(useSessions.getState().byServerRecord[server.id] ?? EMPTY).map((x) => x.name);
}

/** Data curta na língua do app; o ano só quando não é o corrente. */
export function shortDate(iso: string | null | undefined): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const year = d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric';
  return d.toLocaleDateString(intlLocale(), { day: 'numeric', month: 'numeric', year });
}

export function checkedLabel(iso: string | null): string {
  if (!iso) return m.plugins_never_checked();
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return m.plugins_never_checked();
  if (d.toDateString() === new Date().toDateString()) {
    return m.plugins_checked_today({ time: d.toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' }) });
  }
  return m.plugins_checked_on({ date: fmtWhen(d.getTime() / 1000) });
}

/** "<o que mudou> — N sessões abertas ainda estão com o estado anterior", com Reabrir e Depois. */
export function ReopenBanner({ server }: { server: Server }) {
  const n = useLiveClaudeCount(server);
  const changed = usePlugins((s) => s.changed);
  const reopening = usePlugins((s) => s.reopening);
  const refused = usePlugins((s) => s.refused);
  if (!changed || (!n && !refused.length)) return null;
  return <ReopenBannerView n={n} action={changed} refused={refused} reopening={reopening}
    onReopen={() => void usePlugins.getState().reopen(server, liveClaudeNames(server))} onLater={() => usePlugins.getState().dismiss()} />;
}

/** O aviso de reabrir só com props: o `ReopenBanner` liga à loja, a novidade passa dados de exemplo. */
export function ReopenBannerView({ n, action, refused, reopening, onReopen, onLater }: {
  n: number;
  action: string;
  refused: string[];
  reopening: boolean;
  onReopen: () => void;
  onLater: () => void;
}) {
  return <>
    {n ? <InfoNotice text={n === 1 ? m.plugins_reopen_banner_one({ action }) : m.plugins_reopen_banner({ action, n })} /> : null}
    {refused.map((r) => <Notice key={r} text={r} error />)}
    <MachineGroup>
      {n ? <MachineRow title={reopening ? m.plugins_reopening() : n === 1 ? m.plugins_reopen() : m.plugins_reopen_all({ n })} action
        disabled={reopening} onPress={onReopen} /> : null}
      <MachineRow title={m.plugins_reopen_later()} disabled={reopening} onPress={onLater} />
    </MachineGroup>
  </>;
}

/** Linha de plugin instalado: versão, data, pílula de versão nova, Atualizar e a chave de ligar. */
export function InstalledPluginRow({ plugin: p, updating, disabled, onPress, onUpdate, onToggle }: {
  plugin: InstalledPlugin;
  updating: boolean;
  disabled: boolean;
  onPress: () => void;
  onUpdate: () => void;
  onToggle: (on: boolean) => void;
}) {
  return <PluginRow title={p.name} dimmed={!p.enabled}
    pill={p.available_version ? m.plugins_available({ version: p.available_version }) : undefined}
    meta={m.plugins_row_meta({ version: p.version, date: shortDate(p.last_updated), marketplace: p.marketplace })}
    description={p.description}
    onPress={onPress}
    trailing={<>
      {p.available_version ? <RowButton label={updating ? m.plugins_updating() : m.plugins_update()} name={p.name} disabled={disabled} onPress={onUpdate} /> : null}
      <Switch value={p.enabled} disabled={disabled} accessibilityLabel={p.name} onValueChange={onToggle} />
    </>} />;
}

/** Campo de busca/entrada no topo de uma aba. */
export function PluginsField({ icon, ...props }: TextInputProps & { icon: IconName; placeholder: string }) {
  const c = useSettingsColors();
  return <View style={[styles.field, { borderColor: c.border, backgroundColor: c.inset }]}>
    <Icon name={icon} size={14} color={c.faint} />
    <TextInput autoCorrect={false} autoCapitalize="none" clearButtonMode="while-editing" placeholderTextColor={c.faint}
      accessibilityLabel={props.placeholder} {...props} style={[styles.input, { color: c.text }]} />
  </View>;
}

/** Botão pequeno no fim da linha; o alvo de toque chega a 44 pt pelo `hitSlop`. `name`: de quem é, para o leitor de tela. */
export function RowButton({ label, name, onPress, disabled, primary }: { label: string; name?: string; onPress: () => void; disabled?: boolean; primary?: boolean }) {
  const c = useSettingsColors();
  return <Pressable accessibilityRole="button" accessibilityLabel={name ? `${label} ${name}` : label} accessibilityState={{ disabled }} disabled={disabled}
    onPress={onPress} hitSlop={{ top: 8, bottom: 8, left: 4, right: 4 }}
    style={({ pressed }) => [styles.button, { borderColor: c.borderStrong, backgroundColor: primary ? c.accentDim : pressed ? c.hover : 'transparent', opacity: disabled ? 0.5 : 1 }]}>
    <Text style={[styles.buttonText, { color: primary ? c.accentText : c.text }]}>{label}</Text>
  </Pressable>;
}

/** Célula de lista virtualizada desenhada como o cartão do `MachineGroup`: as células juntas formam um só. */
export function PluginListCell({ first, last, children }: { first: boolean; last: boolean; children: ReactNode }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View style={[styles.cell, { borderColor: c.border, backgroundColor: superficie(theme, 0.6) }, first && styles.cellFirst, last && styles.cellLast]}>
    {/* Como no cartão sem cabeçalho: a divisória da primeira linha sobe 1px e some. */}
    <View style={first ? styles.cellBare : undefined}>{children}</View>
  </View>;
}

/** Linha com título, pílula, meta e descrição; `onPress` abre o detalhe, `trailing` fica fora do toque dele. */
export function PluginRow({ title, pill, meta, description, onPress, trailing, dimmed }: {
  title: string;
  pill?: string;
  meta?: string;
  description?: string | null;
  onPress?: () => void;
  trailing?: ReactNode;
  dimmed?: boolean;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const texts = <View style={[styles.texts, dimmed && { opacity: 0.6 }]}>
    <View style={styles.titleLine}>
      <Text style={[styles.title, { color: c.text }]} numberOfLines={1}>{title}</Text>
      {pill ? <Text style={[styles.pill, { color: theme.tokens.status.warning, borderColor: theme.tokens.status.warning }]}>{pill}</Text> : null}
    </View>
    {meta ? <Text style={[styles.meta, { color: c.muted }]} numberOfLines={2}>{meta}</Text> : null}
    {description ? <Text style={[styles.meta, { color: c.faint }]} numberOfLines={2}>{description}</Text> : null}
  </View>;
  return <View style={[styles.row, { borderTopColor: c.border }]}>
    {onPress ? <Pressable accessibilityRole="button" accessibilityLabel={[title, pill].filter(Boolean).join(', ')} accessibilityHint={meta}
      onPress={onPress} style={({ pressed }) => [styles.main, pressed && { opacity: 0.6 }]}>
      {texts}
      {trailing ? null : <Icon name="ChevronRight" size={17} color={c.faint} />}
    </Pressable> : <View style={styles.main}>{texts}</View>}
    {trailing}
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  field: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[3], borderWidth: 1, borderRadius: theme.base.radius.sm },
  input: { flex: 1, minHeight: 44, fontSize: theme.base.text.base, padding: 0 },
  button: { minHeight: 32, justifyContent: 'center', paddingHorizontal: theme.base.space[3], borderWidth: 1, borderRadius: theme.base.radius.sm },
  buttonText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  row: { minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], borderTopWidth: 1 },
  main: { flex: 1, minWidth: 0, minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  texts: { flex: 1, minWidth: 0, gap: theme.base.space[1] },
  titleLine: { flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: theme.base.space[2] },
  title: { flexShrink: 1, fontSize: theme.base.text.base, fontWeight: theme.base.weight.medium },
  pill: { fontSize: theme.base.text.xs, borderWidth: 1, borderRadius: theme.base.radius.full, paddingHorizontal: theme.base.space[2], overflow: 'hidden' },
  meta: { fontSize: theme.base.text.xs, lineHeight: 18 },
  cell: { borderLeftWidth: 1, borderRightWidth: 1, overflow: 'hidden' },
  cellFirst: { borderTopWidth: 1, borderTopLeftRadius: theme.base.radius.md, borderTopRightRadius: theme.base.radius.md },
  cellLast: { borderBottomWidth: 1, borderBottomLeftRadius: theme.base.radius.md, borderBottomRightRadius: theme.base.radius.md },
  cellBare: { marginTop: -1 },
}));

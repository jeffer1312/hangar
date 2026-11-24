import type { ReactNode } from 'react';
import { ActivityIndicator, Pressable, Text, TextInput, View } from 'react-native';
import { SvgXml } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { intlLocale, relativeTime, type WhatsAppState, type WhatsAppTotals } from '@hangar/core';
import { BlockHead, Muted, useInputStyle } from './ServerConfigParts';
import { useSettingsColors, withAlpha } from './colors';
import { STEPS, codeOf, minutesLeft, phrase, stepIndex, type WhatsAppView } from './whatsapp';
import { InstallItems } from '../accounts/InstallItems';
import { superficie } from '../../theme/superficie';
import { Icon, type IconName } from '../../ui/Icon';
import * as m from '../../paraglide/messages';

// Partes da página WhatsApp das Configurações; a miniatura da novidade (features/updates) desenha as mesmas.

export type Act = 'install' | 'update' | 'pair' | 'logout' | 'wipe' | 'uninstall';

// As ferramentas que as sessões usam: nomes do catálogo MCP, iguais nos dois idiomas.
const TOOLS = 'whatsapp_chats · whatsapp_read · whatsapp_search · whatsapp_request_media · whatsapp_draft';
const STEP_LABEL: Record<typeof STEPS[number], () => string> = {
  download: m.native_whatsapp_step_download,
  service: m.native_whatsapp_step_service,
  start: m.native_whatsapp_step_start,
};

const STATUS_ERR: Record<string, () => string> = {
  pair_timeout: m.native_whatsapp_status_err_pair_timeout,
  connect_failed: m.native_whatsapp_status_err_connect_failed,
  pair_code_failed: m.native_whatsapp_status_err_pair_code_failed,
  pair_failed: m.native_whatsapp_status_err_pair_failed,
};

const grouped = (n: number | undefined) => (n ?? 0).toLocaleString(intlLocale());

export function Card({ children }: { children: ReactNode }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  return <View style={[styles.card, { borderColor: c.border, backgroundColor: superficie(theme, 0.6) }]}>{children}</View>;
}

export function Button({ kind = 'primary', label, onPress, disabled = false, loading = false }: {
  kind?: 'primary' | 'ghost' | 'danger'; label: string; onPress: () => void; disabled?: boolean; loading?: boolean;
}) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const danger = theme.tokens.status.error;
  const look = kind === 'primary'
    ? { backgroundColor: disabled ? c.inset : c.accent, borderColor: 'transparent' }
    : { backgroundColor: superficie(theme, 0.6), borderColor: kind === 'danger' ? withAlpha(danger, 0.35) : c.border };
  const color = kind === 'primary' ? (disabled ? c.faint : theme.ink.onAccent) : kind === 'danger' ? danger : c.accentText;
  return <Pressable onPress={onPress} disabled={disabled || loading} accessibilityRole="button" accessibilityLabel={label}
    accessibilityState={{ disabled: disabled || loading, busy: loading }}
    style={({ pressed }) => [styles.button, look, kind !== 'primary' && disabled && { opacity: 0.5 }, pressed && { opacity: 0.8 }]}>
    {loading ? <ActivityIndicator size="small" color={color} /> : null}
    <Text style={[styles.buttonText, { color }, kind !== 'primary' && styles.buttonTextLight]}>{label}</Text>
  </Pressable>;
}

function Link({ label, onPress, disabled }: { label: string; onPress: () => void; disabled?: boolean }) {
  const c = useSettingsColors();
  return <Pressable onPress={onPress} disabled={disabled} accessibilityRole="button" accessibilityLabel={label}
    accessibilityState={{ disabled: !!disabled }} hitSlop={10} style={[styles.link, disabled && { opacity: 0.5 }]}>
    <Text style={[styles.linkText, { color: c.accentText }]}>{label}</Text>
  </Pressable>;
}

export function Check({ checked, label, onToggle, disabled }: { checked: boolean; label: string; onToggle: () => void; disabled?: boolean }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  return <Pressable onPress={onToggle} disabled={disabled} accessibilityRole="checkbox" accessibilityLabel={label}
    accessibilityState={{ checked, disabled: !!disabled }} hitSlop={6} style={styles.check}>
    <View style={[styles.box, checked ? { backgroundColor: c.accent, borderColor: c.accent } : { borderColor: c.borderStrong }]}>
      {checked ? <Icon name="Check" size={14} color={theme.ink.onAccent} /> : null}
    </View>
    <Text style={[styles.checkText, { color: c.text }]}>{label}</Text>
  </Pressable>;
}

/** Faixa de aviso: erro (vermelha) ou atenção (âmbar), com título, detalhe e ação opcionais. */
export function Problem({ tone, icon, title, text, detail, action, onPress, primary, busy, loading }: {
  tone: 'error' | 'warning'; icon: IconName; title?: string; text: string; detail?: string; action?: string; onPress?: () => void;
  primary?: boolean; busy?: boolean; loading?: boolean;
}) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const color = tone === 'error' ? theme.tokens.status.error : theme.tokens.status.warning;
  return <View style={styles.stack}>
    <View accessibilityRole="alert" style={[styles.problem, { borderColor: withAlpha(color, 0.3), backgroundColor: withAlpha(color, 0.08) }]}>
      <Icon name={icon} size={16} color={color} />
      <View style={styles.flex}>
        {title ? <Text style={[styles.problemTitle, { color: c.text }]}>{title}</Text> : null}
        <Text selectable style={[styles.body, { color: title ? c.muted : color }]}>{text}</Text>
        {detail ? <Text selectable style={[styles.small, { color: c.faint }]}>{detail}</Text> : null}
      </View>
    </View>
    {action && onPress ? <Button kind={primary || tone === 'error' ? 'primary' : 'ghost'} label={action} onPress={onPress}
      disabled={busy} loading={loading} /> : null}
  </View>;
}

export function Off({ accepted, busy, onToggle, onInstall }: { accepted: boolean; busy: Act | null; onToggle: () => void; onInstall: () => void }) {
  return <>
    <Muted>{m.native_whatsapp_intro()}</Muted>
    <AcceptRisks accepted={accepted} busy={busy} onToggle={onToggle} />
    <Button label={m.native_whatsapp_install()} onPress={onInstall} disabled={!accepted || !!busy} loading={busy === 'install'} />
  </>;
}

/** Os riscos e a caixa "Entendo os riscos": no desligado e no parado sem aceite gravado. */
export function AcceptRisks({ accepted, busy, onToggle }: { accepted: boolean; busy: Act | null; onToggle: () => void }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const risks: Array<[IconName, string, string, boolean]> = [
    ['TriangleAlert', m.native_whatsapp_risk_unofficial_title(), m.native_whatsapp_risk_unofficial(), true],
    ['Send', m.native_whatsapp_risk_send_title(), m.native_whatsapp_risk_send(), false],
    ['HardDrive', m.native_whatsapp_risk_local_title(), m.native_whatsapp_risk_local(), false],
    ['KeyRound', m.native_whatsapp_risk_who_title(), m.native_whatsapp_risk_who(), false],
  ];
  return <>
    <BlockHead title={m.native_whatsapp_risk_title()} />
    <Card>
      {risks.map(([icon, lead, text, warn], i) => (
        <View key={lead} style={[styles.risk, i > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: c.border }]}>
          <View style={[styles.iconBox, { backgroundColor: c.inset }]}>
            <Icon name={icon} size={15} color={warn ? theme.tokens.status.warning : c.muted} />
          </View>
          <Text style={[styles.body, styles.flex, { color: c.muted }]}>
            <Text style={[styles.strong, { color: warn ? theme.tokens.status.warning : c.text }]}>{lead}</Text> {text}
          </Text>
        </View>
      ))}
    </Card>
    <Check checked={accepted} label={m.native_whatsapp_accept()} onToggle={onToggle} disabled={!!busy} />
  </>;
}

export function Working({ state }: { state: WhatsAppState }) {
  const c = useSettingsColors();
  const update = state.job?.kind === 'update';
  const current = stepIndex(state.job?.step);
  return <>
    <Muted>{update ? m.native_whatsapp_updating_title() : m.native_whatsapp_installing_title()}</Muted>
    <Card>
      <View accessibilityLiveRegion="polite">
        <InstallItems at={current} failed={false} labels={STEPS.map((step) => STEP_LABEL[step]())} />
      </View>
      <View style={[styles.bar, { backgroundColor: c.inset }]}>
        <View style={{ width: `${Math.round(((current + 0.5) / STEPS.length) * 100)}%`, backgroundColor: c.accent }} />
      </View>
    </Card>
  </>;
}

export function Pair({ state, view, busy, qrWanted, asked, phone, phoneError, awaiting, onPhone, onRequestCode, onShowQr, onBackToPhone, onChangeNumber, onCancel, onRetry }: {
  state: WhatsAppState; view: WhatsAppView; busy: Act | null; qrWanted: boolean; asked: boolean; phone: string; phoneError: string; awaiting: boolean;
  onPhone: (t: string) => void; onRequestCode: () => void; onShowQr: () => void; onBackToPhone: () => void; onChangeNumber: () => void;
  onCancel: () => void; onRetry: () => void;
}) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const input = useInputStyle();
  const status = state.status;
  const errorCodeNow = codeOf(status?.error);
  const pairCode = view === 'pairing' ? status?.pair_code : null;
  const qr = view === 'pairing' ? state.qr_svg : null;
  const enough = phone.replace(/\D/g, '').length >= 10;
  const left = minutesLeft(status?.pair_code_expires_at);
  return <>
    <BlockHead title={m.native_whatsapp_pair_title()} />
    {status?.state === 'logged_out' ? <Muted>{m.native_whatsapp_logged_out()}</Muted> : null}
    {errorCodeNow ? <Problem tone="error" icon="CircleAlert" text={phrase(STATUS_ERR, errorCodeNow, m.native_whatsapp_status_err_unknown)} action={m.native_server_retry()}
      busy={!!busy} onPress={onRetry} /> : null}
    {qrWanted ? <>
      <Card>
        <Text style={[styles.body, { color: c.muted }]}>{m.whatsapp_pair_qr_lead()}</Text>
        {[m.native_whatsapp_pair_step1(), m.native_whatsapp_pair_step2(), m.native_whatsapp_pair_step3()].map((t, n) => (
          <Text key={t} style={[styles.body, { color: c.text }]}>{`${n + 1}. ${t}`}</Text>
        ))}
        <View style={styles.qr} accessible accessibilityRole="image" accessibilityLabel={qr ? m.native_whatsapp_pair_step3() : m.native_whatsapp_pair_waiting()}>
          {qr ? <SvgXml xml={qr} width="100%" height="100%" />
            : <Text style={[styles.small, styles.qrWaiting]}>{m.native_whatsapp_pair_waiting()}</Text>}
        </View>
        <Text style={[styles.small, { color: c.muted }]}>{m.native_whatsapp_pair_qr_note()}</Text>
      </Card>
      <Link label={m.whatsapp_pair_back_phone()} onPress={onBackToPhone} disabled={!!busy} />
    </> : asked && pairCode ? <>
      <Card>
        <Text style={[styles.body, { color: c.muted }]}>{m.whatsapp_pair_code_lead()}</Text>
        <Text selectable accessibilityLiveRegion="polite" style={[styles.code, { color: c.text, fontFamily: theme.base.fontMono }]}>{pairCode}</Text>
        <Text style={[styles.small, { color: c.muted }]}>
          {m.native_whatsapp_pair_code_note()}{left !== null ? `${m.whatsapp_pair_code_valid({ n: String(left) })}` : ''}
        </Text>
      </Card>
      <Button kind="ghost" label={m.whatsapp_pair_change_number()} onPress={onChangeNumber} disabled={!!busy} />
    </> : asked && (busy === 'pair' || view === 'pairing' || awaiting) ? <Card>
      <View style={styles.step}>
        <ActivityIndicator size="small" color={c.accent} />
        <Text accessibilityLiveRegion="polite" style={[styles.body, { color: c.muted }]}>{m.native_whatsapp_pair_waiting()}</Text>
      </View>
    </Card> : <>
      <Card>
        <Text style={[styles.body, { color: c.muted }]}>{m.native_whatsapp_pair_phone_hint()}</Text>
        <TextInput {...input} value={phone} onChangeText={onPhone} placeholder={m.native_whatsapp_pair_phone_placeholder()}
          accessibilityLabel={m.native_whatsapp_pair_phone_hint()} keyboardType="phone-pad" textContentType="telephoneNumber"
          autoComplete="tel" autoCorrect={false} returnKeyType="done" editable={!busy}
          onSubmitEditing={() => { if (enough) onRequestCode(); }} />
        {phoneError ? <Text accessibilityRole="alert" style={[styles.small, { color: theme.tokens.status.error }]}>{phoneError}</Text> : null}
        <Button label={m.native_whatsapp_pair_request_code()} onPress={onRequestCode} disabled={!enough || !!busy} loading={busy === 'pair'} />
      </Card>
      <Link label={m.whatsapp_pair_show_qr_other()} onPress={onShowQr} disabled={!!busy} />
    </>}
    <Button kind="ghost" label={m.native_cancel()} onPress={onCancel} disabled={!!busy} />
  </>;
}

export function Connected({ state }: { state: WhatsAppState }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const status = state.status;
  const t: Partial<WhatsAppTotals> = status?.totals ?? {};
  const device = status?.device?.trim();
  const deviceName = status?.device_name?.trim();
  const lastAt = status?.last_message_at ? Date.parse(status.last_message_at) : NaN;
  const since = status?.since ? new Date(status.since) : null;
  const sinceText = since && !Number.isNaN(since.getTime()) ? since.toLocaleDateString(intlLocale()) : null;
  const success = theme.tokens.status.success;
  const danger = theme.tokens.status.error;
  return <>
    <Card>
      <View style={styles.step}>
        <View style={[styles.dot, { backgroundColor: success }]} />
        <Text style={[styles.strong, styles.flex, { color: success }]}>
          {device ? m.native_whatsapp_connected({ device }) : m.native_whatsapp_connected_plain()}
        </Text>
      </View>
      <Text style={[styles.small, { color: c.muted }]}>
        {deviceName ? `${m.native_whatsapp_device_name({ name: deviceName })} ` : ''}
        {Number.isNaN(lastAt) ? m.native_whatsapp_no_message() : m.native_whatsapp_last_message({ ago: relativeTime(lastAt / 1000) })}
      </Text>
    </Card>
    <View style={styles.stats}>
      <Stat value={grouped(t.messages)} label={sinceText ? m.native_whatsapp_total_messages({ date: sinceText }) : m.native_whatsapp_total_messages_plain()} />
      <Stat value={grouped(t.chats)} label={m.native_whatsapp_total_chats({ groups: grouped(t.groups) })} />
      <Stat value={grouped(t.media_saved)} label={m.native_whatsapp_total_media()} />
    </View>
    <BlockHead title={m.native_whatsapp_media_title()} help={m.native_whatsapp_media_desc()} />
    <Card>
      <View style={[styles.bar, { backgroundColor: c.inset }]} accessible={false}>
        {/* flex proporcional: cada fatia na razão do total; sem mídia, a barra fica vazia. */}
        <View style={{ flex: t.media_saved ?? 0, backgroundColor: success }} />
        <View style={{ flex: t.media_phone_only ?? 0, backgroundColor: c.accent }} />
        <View style={{ flex: t.media_lost ?? 0, backgroundColor: danger }} />
      </View>
      <Legend color={success} title={m.whatsapp_media_saved({ n: grouped(t.media_saved) })} />
      <Legend color={c.accent} title={m.native_whatsapp_media_phone_only({ n: grouped(t.media_phone_only) })} hint={m.native_whatsapp_media_phone_only_hint()} />
      <Legend color={danger} title={m.native_whatsapp_media_lost({ n: grouped(t.media_lost) })} hint={m.native_whatsapp_media_lost_hint()} />
    </Card>
    <BlockHead title={m.native_whatsapp_usage_title()} />
    <Card>
      <Text style={[styles.small, { color: c.muted }]}>{m.native_whatsapp_usage_tools()}</Text>
      <Text selectable style={[styles.small, { color: c.text, fontFamily: theme.base.fontMono }]}>{TOOLS}</Text>
      <Text style={[styles.body, { color: c.text }]}>{m.native_whatsapp_usage_skill()}</Text>
      <Text style={[styles.small, { color: c.muted }]}>{m.native_whatsapp_usage_skill_hint()}</Text>
    </Card>
  </>;
}

function Stat({ value, label }: { value: string; label: string }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  return <View style={[styles.stat, { borderColor: c.border, backgroundColor: superficie(theme, 0.6) }]} accessible accessibilityLabel={`${value} ${label}`}>
    <Text style={[styles.statValue, { color: c.text }]} numberOfLines={1} adjustsFontSizeToFit>{value}</Text>
    <Text style={[styles.small, { color: c.muted }]}>{label}</Text>
  </View>;
}

function Legend({ color, title, hint }: { color: string; title: string; hint?: string }) {
  const c = useSettingsColors();
  return <View style={styles.legend}>
    <View style={[styles.dot, styles.legendDot, { backgroundColor: color }]} />
    <View style={styles.flex}>
      <Text style={[styles.body, { color: c.text }]}>{title}</Text>
      {hint ? <Text style={[styles.small, { color: c.muted }]}>{hint}</Text> : null}
    </View>
  </View>;
}

export const styles = StyleSheet.create((theme) => ({
  fill: { flex: 1 },
  flex: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  titleRow: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: theme.base.space[1] },
  stack: { gap: theme.base.space[2] },
  actions: { gap: theme.base.space[2], marginTop: theme.base.space[2] },
  card: { borderRadius: theme.base.radius.md, borderWidth: 1, padding: theme.base.space[4], gap: theme.base.space[2] },
  ghostShort: { height: 90 },
  ghostTall: { height: 150 },
  body: { fontSize: theme.base.text.sm, lineHeight: 20 },
  small: { fontSize: theme.base.text.xs, lineHeight: 18 },
  strong: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  risk: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3], paddingVertical: theme.base.space[2] },
  iconBox: { width: 28, height: 28, borderRadius: theme.base.radius.sm, alignItems: 'center', justifyContent: 'center' },
  // 44 pt: alvo de toque mínimo.
  check: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[1] },
  box: { width: 22, height: 22, borderRadius: theme.base.radius.xs, borderWidth: 2, alignItems: 'center', justifyContent: 'center' },
  checkText: { flex: 1, fontSize: theme.base.text.sm, lineHeight: 20 },
  button: {
    minHeight: 48, borderRadius: theme.base.radius.md, borderWidth: 1, flexDirection: 'row', alignItems: 'center', justifyContent: 'center',
    gap: theme.base.space[2], paddingHorizontal: theme.base.space[4],
  },
  buttonText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  buttonTextLight: { fontWeight: theme.base.weight.medium },
  link: { minHeight: 44, justifyContent: 'center', alignSelf: 'flex-start', paddingHorizontal: theme.base.space[1] },
  linkText: { fontSize: theme.base.text.sm },
  problem: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3], borderWidth: 1, borderRadius: theme.base.radius.md, padding: theme.base.space[3] },
  problemTitle: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  step: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], minHeight: 28 },
  bar: { height: 6, borderRadius: theme.base.radius.full, overflow: 'hidden', flexDirection: 'row', marginVertical: theme.base.space[1] },
  // Fundo branco fixo: o leitor do WhatsApp não lê o QR sobre o tema escuro.
  qr: { width: 200, height: 200, alignSelf: 'center', padding: theme.base.space[2], borderRadius: theme.base.radius.sm, backgroundColor: '#ffffff',
    alignItems: 'center', justifyContent: 'center', marginVertical: theme.base.space[2] },
  qrWaiting: { color: '#000000', textAlign: 'center' },
  code: { fontSize: theme.base.text.xl, letterSpacing: 4, textAlign: 'center', fontWeight: theme.base.weight.semibold, paddingVertical: theme.base.space[2] },
  stats: { flexDirection: 'row', gap: theme.base.space[2] },
  stat: { flex: 1, minWidth: 0, borderWidth: 1, borderRadius: theme.base.radius.md, padding: theme.base.space[3], gap: theme.base.space[0.5] },
  statValue: { fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold },
  dot: { width: 9, height: 9, borderRadius: theme.base.radius.full },
  legend: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[2] },
  legendDot: { marginTop: theme.base.space[1.5] },
  sheet: { paddingHorizontal: theme.base.space[4], gap: theme.base.space[3] },
  sheetTitle: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
}));

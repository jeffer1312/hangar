import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { ActivityIndicator, AppState, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { useIsFocused, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { SvgXml } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  getWhatsApp, installWhatsApp, intlLocale, logoutWhatsApp, pairWhatsApp, relativeTime, uninstallWhatsApp, updateWhatsApp, wipeWhatsApp,
  type Server, type WhatsAppState, type WhatsAppTotals,
} from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader } from '../../src/features/config/PageHeader';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { BlockHead, Chip, Muted, useInputStyle } from '../../src/features/config/ServerConfigParts';
import { useSettingsColors, withAlpha } from '../../src/features/config/colors';
import {
  PAIR_GRACE_MS, POLL_MS, STEPS, codeOf, failedJob, isWhatsAppState, jobError, type JobError, mediaShares, minutesLeft, phoneDigits, secondsSince, shouldPoll, stepIndex, viewOf,
  type WhatsAppView,
} from '../../src/features/config/whatsapp';
import { useWhatsAppSeen } from '../../src/features/config/whatsappSeen';
import { superficie } from '../../src/theme/superficie';
import { Icon, type IconName } from '../../src/ui/Icon';
import { Sheet } from '../../src/ui/Sheet';
import { haptic } from '../../src/ui/haptics';
import { useServers } from '../../src/stores/servers';
import * as m from '../../src/paraglide/messages';

type Act = 'install' | 'update' | 'pair' | 'logout' | 'wipe' | 'uninstall';
type Load = { status: 'loading' } | { status: 'error'; error: string; code?: string } | { status: 'ready'; data: WhatsAppState };
type Confirm = 'logout' | 'wipe' | 'remove';

// As ferramentas que as sessões usam: nomes do catálogo MCP, iguais nos dois idiomas.
const TOOLS = 'whatsapp_chats · whatsapp_read · whatsapp_search · whatsapp_request_media';
const STEP_LABEL: Record<typeof STEPS[number], () => string> = {
  download: m.native_whatsapp_step_download,
  service: m.native_whatsapp_step_service,
  start: m.native_whatsapp_step_start,
};
const ERR: Record<string, () => string> = {
  whatsapp_accept_required: m.native_whatsapp_err_accept_required,
  whatsapp_busy: m.native_whatsapp_err_busy,
  whatsapp_off: m.native_whatsapp_err_off,
  whatsapp_untrusted: m.native_whatsapp_err_untrusted,
};

const errorCode = (e: unknown) => {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === 'string' ? code : undefined;
};

/** Código `whatsapp_*` vira a frase da página (a genérica leva o código); o resto fica com a mensagem do servidor. */
function errorText(e: unknown): string {
  const code = errorCode(e);
  if (code?.startsWith('whatsapp_')) {
    return Object.prototype.hasOwnProperty.call(ERR, code) ? ERR[code]() : m.native_whatsapp_err_unknown({ code });
  }
  return e instanceof Error && e.message ? e.message : m.erro_desconhecido();
}

const STATUS_ERR: Record<string, () => string> = {
  pair_timeout: m.native_whatsapp_status_err_pair_timeout,
  connect_failed: m.native_whatsapp_status_err_connect_failed,
  pair_code_failed: m.native_whatsapp_status_err_pair_code_failed,
  pair_failed: m.native_whatsapp_status_err_pair_failed,
};

const JOB_ERR: Record<JobError, () => string> = {
  download_failed: m.native_whatsapp_job_err_download_failed,
  sem_asset: m.native_whatsapp_job_err_sem_asset,
  sem_checksum: m.native_whatsapp_job_err_sem_checksum,
  hash: m.native_whatsapp_job_err_hash,
  binary_missing: m.native_whatsapp_job_err_binary_missing,
  write_failed: m.native_whatsapp_job_err_write_failed,
  servico_falhou: m.native_whatsapp_job_err_servico_falhou,
  no_release: m.native_whatsapp_job_err_no_release,
  version_unreadable: m.native_whatsapp_job_err_version_unreadable,
  api_too_old: m.native_whatsapp_job_err_api_too_old,
  not_started: m.native_whatsapp_job_err_not_started,
  job_crashed: m.native_whatsapp_job_err_job_crashed,
  update_no_rollback: m.native_whatsapp_job_err_update_no_rollback,
  record_failed: m.native_whatsapp_job_err_record_failed,
  accept_write_failed: m.native_whatsapp_job_err_accept_write_failed,
};

/** O motivo que completa "A instalação parou: …"; código desconhecido leva o código cru. */
const jobErrorText = (code: string) => {
  const known = jobError(code);
  return known ? JOB_ERR[known]() : m.native_whatsapp_job_err_unknown({ code });
};

const statusErrorText = (code: string) =>
  Object.prototype.hasOwnProperty.call(STATUS_ERR, code) ? STATUS_ERR[code]() : m.native_whatsapp_status_err_unknown({ code });

const grouped = (n: number | undefined) => (n ?? 0).toLocaleString(intlLocale());

export default function WhatsApp() {
  const server = useServers((s) => s.active());
  const router = useRouter();
  const focused = useIsFocused();
  const [appActive, setAppActive] = useState(AppState.currentState === 'active');
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [busy, setBusy] = useState<Act | null>(null);
  const [writeError, setWriteError] = useState('');
  const [accepted, setAccepted] = useState(false);
  // O QR dá acesso à conta: só aparece depois que a pessoa pede, nesta visita.
  const [qrWanted, setQrWanted] = useState(false);
  const [asked, setAsked] = useState(false);
  const [phone, setPhone] = useState('');
  const [lastPhone, setLastPhone] = useState('');
  const [phoneError, setPhoneError] = useState('');
  const [awaitUntil, setAwaitUntil] = useState<number | null>(null);
  // Apagou com o serviço parado: o aparelho continua na lista do celular.
  const [unlinkOnPhone, setUnlinkOnPhone] = useState(false);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  // Troca de servidor no meio de uma chamada: a resposta do anterior não pinta esta tela.
  const gen = useRef(0);
  const writeSeq = useRef(0);
  const serverRef = useRef(server);
  serverRef.current = server;

  const read = useCallback(() => {
    const s = serverRef.current;
    if (!s) return;
    const g = gen.current;
    getWhatsApp(s)
      .then((v) => {
        if (g !== gen.current) return;
        setLoad(isWhatsAppState(v) ? { status: 'ready', data: v } : { status: 'error', error: m.native_invalid_response() });
      })
      .catch((e: unknown) => { if (g === gen.current) setLoad({ status: 'error', error: errorText(e), code: errorCode(e) }); });
  }, []);

  useEffect(() => {
    gen.current++;
    setLoad({ status: 'loading' });
    setBusy(null);
    setWriteError('');
    setAccepted(false);
    setQrWanted(false);
    setAsked(false);
    setPhoneError('');
    setAwaitUntil(null);
    setUnlinkOnPhone(false);
    setConfirm(null);
    read();
    return () => { gen.current++; };
  }, [read, server?.id, server?.baseUrl, server?.token]);

  useEffect(() => { useWhatsAppSeen.getState().markSeen(); }, []);

  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      setAppActive(state === 'active');
      if (state === 'active') read();
    });
    return () => sub.remove();
  }, [read]);

  const data = load.status === 'ready' ? load.data : null;
  const view: WhatsAppView | null = data ? viewOf(data) : null;

  // Conectou: o pedido de pareamento acabou.
  useEffect(() => {
    if (view === 'connected') { setQrWanted(false); setAsked(false); setAwaitUntil(null); }
  }, [view]);

  // Job e pareamento mudam sem aviso do servidor: relê a cada 1 s enquanto andam e a tela está à vista.
  useEffect(() => {
    if (!data || !focused || !appActive || busy) return;
    if (!shouldPoll(data, awaitUntil !== null && Date.now() < awaitUntil)) return;
    const t = setTimeout(read, POLL_MS);
    return () => clearTimeout(t);
  }, [data, focused, appActive, busy, awaitUntil, read]);

  const act = (kind: Act, run: (s: Server) => Promise<unknown>) => {
    const s = serverRef.current;
    if (!s || busy) return;
    const g = gen.current;
    const seq = ++writeSeq.current;
    const live = () => g === gen.current && seq === writeSeq.current;
    setBusy(kind);
    setWriteError('');
    if (kind !== 'wipe') setUnlinkOnPhone(false);
    run(s)
      .then((r) => {
        if (!live()) return;
        if (kind === 'wipe') setUnlinkOnPhone((r as { logout?: unknown } | undefined)?.logout === 'not_done');
        if (kind === 'pair') setAwaitUntil(Date.now() + PAIR_GRACE_MS);
        if (kind === 'logout' || kind === 'wipe' || kind === 'uninstall') haptic.success();
      })
      .catch((e: unknown) => {
        if (!live()) return;
        // Outro job já em curso: a página passa a acompanhar esse.
        if ((kind === 'install' || kind === 'update') && errorCode(e) === 'whatsapp_busy') return;
        setWriteError(errorText(e));
        haptic.error();
      })
      .finally(() => {
        if (!live()) return;
        setBusy(null);
        // A ação pode ter chegado antes da queda; a releitura mostra o estado real.
        read();
      });
  };

  const install = () => {
    if (!accepted && !data?.accepted_at) return;
    act('install', installWhatsApp);
  };
  const requestCode = (digitsOverride?: string) => {
    const digits = digitsOverride ?? phoneDigits(phone);
    if (!digits) { setPhoneError(m.native_whatsapp_pair_phone_invalid()); haptic.error(); return; }
    setPhoneError('');
    setQrWanted(false);
    setAsked(true);
    setLastPhone(digits);
    act('pair', (s) => pairWhatsApp(s, digits));
  };
  /** Já em pareamento com QR, só mostra o que o servidor mandou: pedir de novo trocaria o código. */
  const showQr = () => {
    setAsked(false);
    setQrWanted(true);
    if (view === 'pairing' && data?.qr_svg && !data.status?.error) read();
    else act('pair', (s) => pairWhatsApp(s));
  };
  const cancelPair = () => {
    if (!qrWanted && !asked) { router.back(); return; }
    setQrWanted(false);
    setAsked(false);
    setAwaitUntil(null);
    setPhoneError('');
  };

  const confirmed = (kind: Confirm, logoutFirst: boolean) => {
    setConfirm(null);
    if (kind === 'logout') act('logout', logoutWhatsApp);
    else if (kind === 'wipe') act('wipe', wipeWhatsApp);
    else act('uninstall', (s) => uninstallWhatsApp(s, logoutFirst));
  };

  let body: ReactNode;
  if (!server) body = <InfoNotice text={m.native_settings_offline()} />;
  else if (load.status === 'loading') body = <Loading />;
  else if (load.status === 'error') body = <Problem tone="error" icon="CircleAlert" text={load.error} detail={load.code}
    action={m.native_server_retry()} onPress={() => { setLoad({ status: 'loading' }); read(); }} />;
  else {
    const d = load.data;
    const v = viewOf(d);
    const failed = v === 'working' ? null : failedJob(d);
    body = <>
      {failed ? <Problem tone="error" icon="CircleAlert"
        text={(failed.kind === 'update' ? m.native_whatsapp_update_failed : m.native_whatsapp_install_failed)({
          reason: jobErrorText(failed.code) })}
        // Reinstalar exige o aceite, o gravado no servidor ou o marcado agora.
        action={m.native_server_retry()} busy={!!busy || (failed.kind !== 'update' && !accepted && !d.accepted_at)}
        loading={busy === (failed.kind === 'update' ? 'update' : 'install')}
        onPress={() => (failed.kind === 'update' ? act('update', updateWhatsApp) : install())} /> : null}
      {v === 'off' ? <Off accepted={accepted} busy={busy} onToggle={() => { haptic.selection(); setAccepted((a) => !a); }} onInstall={install} />
        : v === 'working' ? <Working state={d} />
        : v === 'stopped' ? <Problem tone="warning" icon="TriangleAlert" title={m.native_whatsapp_stopped_title()} text={m.native_whatsapp_stopped_body()}
          action={m.native_server_retry()} busy={!!busy} loading={busy === 'install'} onPress={() => act('install', installWhatsApp)} />
        : v === 'unreachable' || v === 'unknown' ? <Problem tone="warning" icon="TriangleAlert" title={m.native_whatsapp_unreachable_title()}
          text={v === 'unreachable' ? m.native_whatsapp_unreachable_body() : m.native_invalid_response()}
          action={m.native_server_retry()} busy={!!busy} onPress={read} />
        : v === 'outdated' ? <Problem tone="warning" icon="TriangleAlert" title={m.native_whatsapp_outdated_title()} text={m.native_whatsapp_outdated_body()}
          action={m.native_whatsapp_update()} primary busy={!!busy} loading={busy === 'update'} onPress={() => act('update', updateWhatsApp)} />
        : v === 'connected' ? <Connected state={d} />
        : <Pair state={d} view={v} busy={busy} qrWanted={qrWanted} asked={asked} phone={phone} phoneError={phoneError}
          awaiting={awaitUntil !== null && Date.now() < awaitUntil}
          onPhone={(t) => { setPhone(t); setPhoneError(''); }} onRequestCode={() => requestCode()} onShowQr={showQr}
          onBackToPhone={() => setQrWanted(false)} onChangeNumber={() => setAsked(false)} onCancel={cancelPair}
          onRetry={() => (asked && lastPhone ? requestCode(lastPhone) : showQr())} />}
      {d.installed && v !== 'working' ? <View style={styles.actions}>
        {v === 'connected' ? <Button kind="ghost" label={m.native_whatsapp_disconnect()} disabled={!!busy} loading={busy === 'logout'}
          onPress={() => setConfirm('logout')} /> : null}
        <Button kind="danger" label={m.native_whatsapp_wipe()} disabled={!!busy} loading={busy === 'wipe'} onPress={() => setConfirm('wipe')} />
        <Button kind="ghost" label={m.native_whatsapp_remove()} disabled={!!busy} loading={busy === 'uninstall'} onPress={() => setConfirm('remove')} />
      </View> : null}
    </>;
  }

  return (
    <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.fill}>
      <Pagina>
        <View style={styles.titleRow}>
          <PageHeader title={m.native_settings_page_whatsapp()} />
          <Chip text={m.native_server_scope()} />
        </View>
        {writeError ? <Problem tone="error" icon="CircleAlert" text={writeError} /> : null}
        {unlinkOnPhone ? <Problem tone="warning" icon="TriangleAlert" text={m.native_whatsapp_unlink_on_phone()} /> : null}
        {body}
      </Pagina>
      {confirm && data ? <ConfirmSheet kind={confirm} running={data.service === 'running'} connected={view === 'connected'}
        onConfirm={confirmed} onClose={() => setConfirm(null)} /> : null}
    </KeyboardAvoidingView>
  );
}

function Loading() {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const box = { borderColor: c.border, backgroundColor: superficie(theme, 0.6) };
  return <View style={styles.stack} accessibilityRole="progressbar" accessibilityLabel={m.native_server_loading()}>
    <Muted>{m.native_server_loading()}</Muted>
    <View style={[styles.card, styles.ghostShort, box, { opacity: 0.5 }]} />
    <View style={[styles.card, styles.ghostTall, box, { opacity: 0.35 }]} />
  </View>;
}

function Card({ children }: { children: ReactNode }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  return <View style={[styles.card, { borderColor: c.border, backgroundColor: superficie(theme, 0.6) }]}>{children}</View>;
}

function Button({ kind = 'primary', label, onPress, disabled = false, loading = false }: {
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

function Check({ checked, label, onToggle, disabled }: { checked: boolean; label: string; onToggle: () => void; disabled?: boolean }) {
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
function Problem({ tone, icon, title, text, detail, action, onPress, primary, busy, loading }: {
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

function Off({ accepted, busy, onToggle, onInstall }: { accepted: boolean; busy: Act | null; onToggle: () => void; onInstall: () => void }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const risks: Array<[IconName, string, string, boolean]> = [
    ['TriangleAlert', m.native_whatsapp_risk_unofficial_title(), m.native_whatsapp_risk_unofficial(), true],
    ['Eye', m.native_whatsapp_risk_readonly_title(), m.native_whatsapp_risk_readonly(), false],
    ['HardDrive', m.native_whatsapp_risk_local_title(), m.native_whatsapp_risk_local(), false],
    ['KeyRound', m.native_whatsapp_risk_who_title(), m.native_whatsapp_risk_who(), false],
  ];
  return <>
    <Muted>{m.native_whatsapp_intro()}</Muted>
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
    <Button label={m.native_whatsapp_install()} onPress={onInstall} disabled={!accepted || !!busy} loading={busy === 'install'} />
  </>;
}

function Working({ state }: { state: WhatsAppState }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const update = state.job?.kind === 'update';
  const current = stepIndex(state.job?.step);
  return <>
    <Muted>{update ? m.native_whatsapp_updating_title() : m.native_whatsapp_installing_title()}</Muted>
    <Card>
      <View accessibilityLiveRegion="polite" style={styles.stackTight}>
        {STEPS.map((step, n) => {
          const done = n < current;
          const running = n === current;
          return <View key={step} style={styles.step}>
            <View style={styles.stepIcon}>
              {running ? <ActivityIndicator size="small" color={c.accent} />
                : <Icon name={done ? 'CircleCheck' : 'Circle'} size={16} color={done ? theme.tokens.status.success : c.faint} />}
            </View>
            <Text style={[styles.body, { color: done || running ? c.text : c.muted }]}>{STEP_LABEL[step]()}</Text>
          </View>;
        })}
      </View>
      <View style={[styles.bar, { backgroundColor: c.inset }]}>
        <View style={{ width: `${Math.round(((current + 0.5) / STEPS.length) * 100)}%`, backgroundColor: c.accent }} />
      </View>
    </Card>
  </>;
}

function Pair({ state, view, busy, qrWanted, asked, phone, phoneError, awaiting, onPhone, onRequestCode, onShowQr, onBackToPhone, onChangeNumber, onCancel, onRetry }: {
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
    {errorCodeNow ? <Problem tone="error" icon="CircleAlert" text={statusErrorText(errorCodeNow)} action={m.native_server_retry()}
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

function Connected({ state }: { state: WhatsAppState }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const status = state.status;
  const t: Partial<WhatsAppTotals> = status?.totals ?? {};
  const device = status?.device?.trim();
  const deviceName = status?.device_name?.trim();
  const ago = secondsSince(status?.last_message_at);
  const since = status?.since ? new Date(status.since) : null;
  const sinceText = since && !Number.isNaN(since.getTime()) ? since.toLocaleDateString(intlLocale()) : null;
  const shares = mediaShares(t);
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
        {ago === null ? m.native_whatsapp_no_message()
          : m.native_whatsapp_last_message({ ago: relativeTime(Date.parse(status!.last_message_at!) / 1000) })}
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
        <View style={{ flex: shares.saved, backgroundColor: success }} />
        <View style={{ flex: shares.phoneOnly, backgroundColor: c.accent }} />
        <View style={{ flex: shares.lost, backgroundColor: danger }} />
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

/** Folha de confirmação: Remover oferece desconectar antes, porque depois só o celular tira o aparelho da lista. */
function ConfirmSheet({ kind, running, connected, onConfirm, onClose }: {
  kind: Confirm; running: boolean; connected: boolean; onConfirm: (kind: Confirm, logoutFirst: boolean) => void; onClose: () => void;
}) {
  const insets = useSafeAreaInsets();
  const c = useSettingsColors();
  const [logoutFirst, setLogoutFirst] = useState(connected);
  const [title, text, ok] = kind === 'logout'
    ? [m.native_whatsapp_disconnect_title(), m.native_whatsapp_disconnect_body(), m.native_whatsapp_disconnect()]
    : kind === 'wipe'
      ? [m.native_whatsapp_wipe_title(), running ? m.native_whatsapp_wipe_body() : m.native_whatsapp_wipe_body_stopped(), m.native_whatsapp_wipe_ok()]
      : [m.native_whatsapp_remove_title(), m.native_whatsapp_remove_body(), m.native_whatsapp_remove_ok()];
  return <Sheet open onDismiss={onClose}>
    <ScrollView contentContainerStyle={[styles.sheet, { paddingBottom: insets.bottom + 24 }]}>
      <Text accessibilityRole="header" style={[styles.sheetTitle, { color: c.text }]}>{title}</Text>
      <Text style={[styles.body, { color: c.muted }]}>{text}</Text>
      {kind === 'remove' && connected ? <Check checked={logoutFirst} label={m.native_whatsapp_remove_logout()}
        onToggle={() => { haptic.selection(); setLogoutFirst((v) => !v); }} /> : null}
      <Button kind={kind === 'logout' ? 'primary' : 'danger'} label={ok} onPress={() => onConfirm(kind, kind === 'remove' && connected && logoutFirst)} />
      <Button kind="ghost" label={m.native_cancel()} onPress={onClose} />
    </ScrollView>
  </Sheet>;
}

const styles = StyleSheet.create((theme) => ({
  fill: { flex: 1 },
  flex: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  titleRow: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: theme.base.space[1] },
  stack: { gap: theme.base.space[2] },
  stackTight: { gap: theme.base.space[1] },
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
  stepIcon: { width: 20, alignItems: 'center' },
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

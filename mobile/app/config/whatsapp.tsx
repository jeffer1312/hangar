import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { AppState, ScrollView, Text, View } from 'react-native';
import { useIsFocused, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useUnistyles } from 'react-native-unistyles';
import {
  getWhatsApp, installWhatsApp, logoutWhatsApp, pairWhatsApp, uninstallWhatsApp, updateWhatsApp, wipeWhatsApp,
  type Server, type WhatsAppState,
} from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader } from '../../src/features/config/PageHeader';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { Chip, Muted } from '../../src/features/config/ServerConfigParts';
import { useSettingsColors } from '../../src/features/config/colors';
import {
  PAIR_GRACE_MS, POLL_MS, failedJob, isWhatsAppState, phoneDigits, phrase, shouldPoll, viewOf, type WhatsAppView,
} from '../../src/features/config/whatsapp';
import { Button, Check, Connected, Off, Pair, Problem, Working, styles, type Act } from '../../src/features/config/WhatsAppParts';
import { useWhatsAppSeen } from '../../src/features/config/whatsappSeen';
import { superficie } from '../../src/theme/superficie';
import { Sheet } from '../../src/ui/Sheet';
import { haptic } from '../../src/ui/haptics';
import { useServers } from '../../src/stores/servers';
import * as m from '../../src/paraglide/messages';

type Load = { status: 'loading' } | { status: 'error'; error: string; code?: string } | { status: 'ready'; data: WhatsAppState };
type Confirm = 'logout' | 'wipe' | 'remove';

const ERR: Record<string, () => string> = {
  accept_required: m.native_whatsapp_err_accept_required,
  busy: m.native_whatsapp_err_busy,
  off: m.native_whatsapp_err_off,
  untrusted: m.native_whatsapp_err_untrusted,
  wipe_partial: m.native_whatsapp_err_wipe_partial,
  wipe_failed: m.native_whatsapp_err_wipe_failed,
  service_starting: m.native_whatsapp_err_service_starting,
  not_installed: m.native_whatsapp_err_not_installed,
  unreachable: m.native_whatsapp_err_unreachable,
  accept_write_failed: m.native_whatsapp_err_accept_write_failed,
  not_paired: m.native_whatsapp_err_not_paired,
  outdated: m.native_whatsapp_err_outdated,
  remove_failed: m.native_whatsapp_err_remove_failed,
  servico_falhou: m.native_whatsapp_err_servico_falhou,
  bad_reply: m.native_whatsapp_err_bad_reply,
  error: m.native_whatsapp_err_error,
  owner_only: m.native_whatsapp_err_owner_only,
};

const errorCode = (e: unknown) => {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === 'string' ? code : undefined;
};

/** Código `whatsapp_*` vira a frase da página (a genérica leva o código); o resto fica com a mensagem do servidor. */
function errorText(e: unknown): string {
  const code = errorCode(e);
  if (code?.startsWith('whatsapp_')) return phrase(ERR, code, m.native_whatsapp_err_unknown);
  return e instanceof Error && e.message ? e.message : m.erro_desconhecido();
}

/** O motivo que completa "A instalação parou: …". */
const JOB_ERR: Record<string, () => string> = {
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

  /** O pedido de pareamento acabou: QR escondido, número fora da tela, sem espera. */
  const resetPairing = useCallback(() => {
    setQrWanted(false);
    setAsked(false);
    setAwaitUntil(null);
    setPhoneError('');
  }, []);

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
    resetPairing();
    setUnlinkOnPhone(false);
    setConfirm(null);
    read();
    return () => { gen.current++; };
  }, [read, resetPairing, server?.id, server?.baseUrl, server?.token]);

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
    if (view === 'connected') resetPairing();
  }, [view, resetPairing]);

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
    resetPairing();
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
  else if (data && view) {
    const failed = view === 'working' ? null : failedJob(data);
    body = <>
      {failed ? <Problem tone="error" icon="CircleAlert"
        text={(failed.kind === 'update' ? m.native_whatsapp_update_failed : m.native_whatsapp_install_failed)({
          reason: phrase(JOB_ERR, failed.code, m.native_whatsapp_job_err_unknown) })}
        // Reinstalar exige o aceite, o gravado no servidor ou o marcado agora.
        action={m.native_server_retry()} busy={!!busy || (failed.kind !== 'update' && !accepted && !data.accepted_at)}
        loading={busy === (failed.kind === 'update' ? 'update' : 'install')}
        onPress={() => (failed.kind === 'update' ? act('update', updateWhatsApp) : install())} /> : null}
      {view === 'off' ? <Off accepted={accepted} busy={busy} onToggle={() => { haptic.selection(); setAccepted((a) => !a); }} onInstall={install} />
        : view === 'working' ? <Working state={data} />
        : view === 'stopped' ? <Problem tone="warning" icon="TriangleAlert" title={m.native_whatsapp_stopped_title()} text={m.native_whatsapp_stopped_body()}
          action={m.native_server_retry()} busy={!!busy} loading={busy === 'install'} onPress={() => act('install', installWhatsApp)} />
        : view === 'schema_newer' ? <Problem tone="warning" icon="TriangleAlert" title={m.native_whatsapp_schema_newer_title()}
          text={m.native_whatsapp_schema_newer_body()}
          action={m.native_whatsapp_update()} primary busy={!!busy} loading={busy === 'update'} onPress={() => act('update', updateWhatsApp)} />
        : view === 'unreachable' || view === 'unknown' ? <Problem tone="warning" icon="TriangleAlert" title={m.native_whatsapp_unreachable_title()}
          text={view === 'unreachable' ? m.native_whatsapp_unreachable_body() : m.native_invalid_response()}
          action={m.native_server_retry()} busy={!!busy} onPress={read} />
        : view === 'outdated' ? <Problem tone="warning" icon="TriangleAlert" title={m.native_whatsapp_outdated_title()} text={m.native_whatsapp_outdated_body()}
          action={m.native_whatsapp_update()} primary busy={!!busy} loading={busy === 'update'} onPress={() => act('update', updateWhatsApp)} />
        : view === 'connected' ? <Connected state={data} />
        : <Pair state={data} view={view} busy={busy} qrWanted={qrWanted} asked={asked} phone={phone} phoneError={phoneError}
          awaiting={awaitUntil !== null && Date.now() < awaitUntil}
          onPhone={(t) => { setPhone(t); setPhoneError(''); }} onRequestCode={() => requestCode()} onShowQr={showQr}
          onBackToPhone={() => setQrWanted(false)} onChangeNumber={() => setAsked(false)} onCancel={cancelPair}
          onRetry={() => (asked && lastPhone ? requestCode(lastPhone) : showQr())} />}
      {data.installed && view !== 'working' ? <View style={styles.actions}>
        {view === 'connected' ? <Button kind="ghost" label={m.native_whatsapp_disconnect()} disabled={!!busy} loading={busy === 'logout'}
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

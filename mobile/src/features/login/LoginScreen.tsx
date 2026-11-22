import { useEffect, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { KeyboardAwareScrollView } from 'react-native-keyboard-controller';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { parseInviteLink, type Server } from '@hangar/core';
import { Screen } from '../../ui/Screen';
import { Glass } from '../../ui/Glass';
import { Sheet } from '../../ui/Sheet';
import { Icon } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { LoginField, LoginSubmit } from './LoginFields';
import { QrScanner } from './QrScanner';
import { parsePairing } from './pairing';
import { AddressForm } from './AddressForm';
import { GuestPicker } from './GuestPicker';
import { ConnectMark } from '../connect/ConnectHero';
import { ConnectBoundary } from '../connect/ConnectBoundary';
import { ConnectMachines } from '../connect/ConnectMachines';
import { useConnect } from '../connect/connectStore';
import { Notice } from '../machines/parts';
import { InvitePanel } from '../share/InvitePanel';
import { useServers } from '../../stores/servers';
import { DEMO_BASE_URL, DEMO_TOKEN } from '../../net/demo';
import * as m from '../../paraglide/messages';

type Guest = { user: string; servers: Server[] };

/** "Entrar no Hangar": conta do Connect ou usuário de convidada no mesmo campo; o resto em "Outras formas de entrar". */
export function LoginScreen() {
  const router = useRouter();
  const { theme } = useUnistyles();
  const withoutServers = useServers((s) => s.servers.length === 0);
  const signedIn = useConnect((s) => !!s.account);
  const token = useConnect((s) => s.account?.token ?? '');
  const notice = useConnect((s) => s.notice);
  const [name, setName] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  // A conta só aparece depois de entrar nesta tela: quem veio buscar o QR não cai na lista de máquinas.
  const [accountOpen, setAccountOpen] = useState(false);
  const [guest, setGuest] = useState<Guest | null>(null);
  const [others, setOthers] = useState(false);
  const [manual, setManual] = useState<{ base: string; token: string } | null>(null);
  const [scanError, setScanError] = useState('');
  const [scanning, setScanning] = useState(false);
  const [inviteOpen, setInviteOpen] = useState(false);
  const [inviteLink, setInviteLink] = useState('');
  useEffect(() => { void useConnect.getState().load(); }, []);

  const submit = async () => {
    if (busy || !name.trim() || !password) return;
    setBusy(true);
    setError('');
    try {
      // A conta guardada é lida antes: lida depois, a leitura atrasada apagaria a que acabou de abrir.
      await useConnect.getState().load();
      const found = await useConnect.getState().signInAnywhere(name, password);
      haptic.success();
      setPassword('');
      if (found.kind === 'guest') setGuest({ user: name.trim().toLowerCase(), servers: found.servers });
      else setAccountOpen(true);
    } catch (e) {
      haptic.error();
      setError(e instanceof Error && e.message ? e.message : m.login_falha());
    } finally {
      setBusy(false);
    }
  };

  const enterGuest = (picked: Server[]) => {
    useServers.getState().appendImported(picked);
    haptic.success();
    router.replace('/');
  };

  const openDemo = () => {
    useServers.getState().add({ baseUrl: DEMO_BASE_URL, token: DEMO_TOKEN, label: m.mobile_demo_server() });
    router.replace('/');
  };

  const handleScan = (texto: string) => {
    const cru = texto.trim();
    setScanning(false);
    setScanError('');
    if (parseInviteLink(cru)) {
      setInviteLink(cru);
      setInviteOpen(true);
      return;
    }
    const p = parsePairing(cru);
    if (!p) {
      setScanError(cru.includes('://') ? m.login_url_token_invalidos() : m.login_qr_sem_url());
      return;
    }
    setManual({ base: p.base, token: p.token });
  };

  const body = guest
    ? <GuestPicker user={guest.user} servers={guest.servers} onEnter={enterGuest} onLeave={() => setGuest(null)} />
    : accountOpen && signedIn
      ? <ConnectBoundary resetKey={token}>
        <View style={styles.machines}>
          <Notice text={notice} />
          <ConnectMachines onDone={() => router.replace('/')} />
        </View>
      </ConnectBoundary>
      : <Glass variant="modal" style={styles.card}>
        <View style={styles.header}>
          <ConnectMark size={64} />
          <Text accessibilityRole="header" style={styles.title}>{m.login_hangar_title()}</Text>
          <Text style={styles.subtitle}>{m.login_hangar_subtitle()}</Text>
        </View>
        <View style={styles.form}>
          <LoginField label={m.login_account_or_user()} value={name} onChangeText={setName} placeholder={m.login_account_or_user_hint()}
            editable={!busy} spellCheck={false} textContentType="username" autoComplete="username" returnKeyType="next" />
          <LoginField label={m.login_senha()} value={password} onChangeText={setPassword} placeholder="••••••••" editable={!busy}
            secureTextEntry textContentType="password" autoComplete="current-password" returnKeyType="go" onSubmitEditing={() => { void submit(); }} />
          {error ? <View style={styles.errorBox}><Text accessibilityRole="alert" style={styles.errorText}>{error}</Text></View> : null}
          <LoginSubmit label={m.login_entrar()} text={busy ? m.login_entrando() : m.login_entrar()} blocked={busy || !name.trim() || !password}
            onPress={() => { void submit(); }} />
          {/* A senha da convidada é derivada no aparelho, o que leva tempo no celular. */}
          {busy ? <Text accessibilityRole="alert" style={styles.waitText}>{m.mobile_sync_login_wait()}</Text> : null}
          <Pressable accessibilityRole="button" accessibilityLabel={m.connect_create_free()} disabled={busy} style={styles.link}
            onPress={() => { haptic.selection(); router.push('/connect?mode=create' as never); }}>
            <Text style={[styles.linkText, { color: theme.tokens.accent.base }]}>{m.connect_create_free()}</Text>
          </Pressable>
        </View>
        <View style={styles.others}>
          <Pressable accessibilityRole="button" accessibilityLabel={m.login_other_ways()} accessibilityState={{ expanded: others }}
            style={styles.othersToggle} onPress={() => setOthers(!others)}>
            <Icon name={others ? 'ChevronDown' : 'ChevronRight'} size={16} color={theme.tokens.text.muted} />
            <Text style={styles.othersText}>{m.login_other_ways()}</Text>
          </Pressable>
          {others ? <>
            <Other label={m.login_qr()} onPress={() => setScanning(true)} />
            {scanError ? <Text accessibilityRole="alert" style={styles.errorText}>{scanError}</Text> : null}
            <Other label={m.login_address_token()} expanded={!!manual}
              onPress={() => setManual(manual ? null : { base: '', token: '' })} />
            {manual ? <AddressForm key={`${manual.base}|${manual.token}`} base={manual.base} token={manual.token} /> : null}
            <Other label={m.mobile_share_invite_open()} onPress={() => { setInviteLink(''); setInviteOpen(true); }} />
            {withoutServers ? <Other label={m.mobile_demo_open()} onPress={openDemo} /> : null}
          </> : null}
        </View>
      </Glass>;

  return (
    <Screen>
      <KeyboardAwareScrollView contentContainerStyle={styles.container} keyboardShouldPersistTaps="handled" bottomOffset={16}>
        <View style={styles.column}>{body}</View>
      </KeyboardAwareScrollView>

      {/* `scrollable` é o que faz a sheet dar a altura do detent ao conteúdo; sem ele o flex:1 da câmera colapsa numa faixa. */}
      <Sheet open={scanning} sizes={['large']} scrollable onDismiss={() => setScanning(false)}>
        <QrScanner onScan={handleScan} onClose={() => setScanning(false)} />
      </Sheet>
      <Sheet open={inviteOpen} sizes={['auto', 'large']} scrollable onDismiss={() => setInviteOpen(false)}>
        {inviteOpen ? <View style={styles.inviteForm}>
          <InvitePanel key={inviteLink} initialLink={inviteLink}
            onAccepted={(server, session) => router.replace(`/s/${encodeURIComponent(server.id)}/${encodeURIComponent(session)}`)} />
          <Other label={m.comum_cancelar()} onPress={() => setInviteOpen(false)} />
        </View> : null}
      </Sheet>
    </Screen>
  );
}

function Other({ label, onPress, expanded }: { label: string; onPress: () => void; expanded?: boolean }) {
  return <Pressable style={styles.otherBtn} onPress={onPress} accessibilityRole="button" accessibilityLabel={label}
    accessibilityState={expanded === undefined ? undefined : { expanded }}>
    <Text style={styles.otherText}>{label}</Text>
  </Pressable>;
}

const styles = StyleSheet.create((theme) => ({
  container: {
    flexGrow: 1,
    alignItems: 'center',
    paddingTop: theme.base.space[10],
    paddingHorizontal: theme.base.space[6],
    paddingBottom: theme.base.space[6],
  },
  column: { width: '100%', maxWidth: 400 },
  machines: { gap: theme.base.space[4] },
  inviteForm: { padding: theme.base.space[4], gap: theme.base.space[3] },
  card: { width: '100%', padding: theme.base.space[6], gap: theme.base.space[5] },
  header: { alignItems: 'center', gap: theme.base.space[2] },
  title: { ...theme.type.title, color: theme.tokens.text.primary, textAlign: 'center' },
  subtitle: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center', lineHeight: 21 },
  form: { gap: theme.base.space[4] },
  errorBox: {
    backgroundColor: theme.tokens.status.error + '14',
    borderWidth: 1,
    borderColor: theme.tokens.status.error + '33',
    borderRadius: theme.base.radius.sm,
    padding: theme.base.space[3],
  },
  errorText: { fontSize: theme.base.text.sm, color: theme.tokens.status.error },
  waitText: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center' },
  link: { minHeight: 44, justifyContent: 'center', alignItems: 'center' },
  linkText: { ...theme.type.control },
  others: { gap: theme.base.space[2], borderTopWidth: 1, borderTopColor: theme.tokens.border.default, paddingTop: theme.base.space[2] },
  othersToggle: { minHeight: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: theme.base.space[1.5] },
  othersText: { ...theme.type.label, color: theme.tokens.text.muted },
  otherBtn: {
    minHeight: 48,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    justifyContent: 'center',
    alignItems: 'center',
  },
  otherText: { color: theme.tokens.text.secondary, fontSize: theme.base.text.base, fontWeight: theme.base.weight.medium },
}));

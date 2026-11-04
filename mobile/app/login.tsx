import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { KeyboardAwareScrollView } from 'react-native-keyboard-controller';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Screen } from '../src/ui/Screen';
import { Glass } from '../src/ui/Glass';
import { Sheet } from '../src/ui/Sheet';
import { LoginField, LoginSubmit } from '../src/features/login/LoginFields';
import { QrScanner } from '../src/features/login/QrScanner';
import { parsePairing } from '../src/features/login/pairing';
import { syncLogin } from '../src/features/login/syncLogin';
import { useServers } from '../src/stores/servers';
import { parseInviteLink } from '@hangar/core';
import { verifyLoginMachine } from '../src/features/machines/loginMachine';
import { InvitePanel } from '../src/features/share/InvitePanel';
import * as m from '../src/paraglide/messages';

export default function Login() {
  const router = useRouter();
  const [baseUrl, setBaseUrl] = useState('');
  const [token, setToken] = useState('');
  const [withPassword, setWithPassword] = useState(false);
  const [user, setUser] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [inviteOpen, setInviteOpen] = useState(false);
  const [inviteLink, setInviteLink] = useState('');

  const handleScan = (texto: string) => {
    const cru = texto.trim();
    setScanning(false);
    if (parseInviteLink(cru)) {
      setInviteLink(cru);
      setInviteOpen(true);
      setError('');
      return;
    }
    const p = parsePairing(cru);
    if (!p) {
      setError(cru.includes('://') ? m.login_url_token_invalidos() : m.login_qr_sem_url());
      return;
    }
    setBaseUrl(p.base);
    setToken(p.token);
    setWithPassword(false);
    setError('');
  };

  const signIn = async () => {
    setLoading(true);
    setError('');
    try {
      const servers = await syncLogin(baseUrl.trim(), user.trim(), password);
      if (!servers.length) {
        setError(m.mobile_sync_login_empty());
        return;
      }
      // Máquina já pareada fica como está: o cofre pode trazer token velho; token que morreu sai pelo 401.
      useServers.getState().appendImported(servers);
      router.replace('/');
    } catch (e) {
      setError(e instanceof Error && e.message ? e.message : m.login_falha());
    } finally {
      setLoading(false);
    }
  };

  const connect = async () => {
    const base = baseUrl.trim();
    const tok = token.trim();
    // Monta texto de pareamento e valida pelo mesmo parsePairing do QR (regra única)
    const cru = base + (base.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(tok);
    const p = parsePairing(cru);
    if (!p) {
      setError(!base || !tok ? m.login_informe_url_token() : m.login_url_token_invalidos());
      return;
    }

    setLoading(true);
    setError('');

    // add é síncrono em memória, mas persist é async (emenda 2) — o probe só roda depois
    const existing = useServers.getState().servers.some((s) => s.baseUrl === p.base && !s.invite);
    const added = useServers.getState().add({ baseUrl: p.base, token: p.token, label: m.native_settings_group_server() });

    try {
      await verifyLoginMachine(added, !existing);
      router.replace('/');
    } catch (e) {

      // rollback: remove o servidor que acabamos de adicionar (mesmo que persist falhe, remove da memória)
      useServers.getState().remove(added.id);
      const status = (e as any)?.status;
      if (status === 401) {
        setError(m.login_url_token_invalidos());
      } else {
        const msg = e instanceof Error && e.message ? e.message : m.login_falha();
        setError(msg ? `${m.login_falha()}: ${msg}` : m.login_url_token_invalidos());
      }
    } finally {
      setLoading(false);
    }
  };

  const blocked = loading || (withPassword ? !baseUrl.trim() || !user.trim() || !password : !token.trim());
  const submitLabel = withPassword ? m.login_entrar() : m.login_conectar();

  return (
    <Screen>
      <KeyboardAwareScrollView contentContainerStyle={styles.container} keyboardShouldPersistTaps="handled" bottomOffset={16}>
        <Glass variant="modal" style={styles.card}>
          <View style={styles.header}>
            <Text style={styles.title}>{m.login_titulo()}</Text>
            <Text style={styles.tagline}>{m.login_tagline()}</Text>
          </View>

          <View style={styles.form}>
            <LoginField
              label={m.sessao_url_servidor()}
              value={baseUrl}
              onChangeText={setBaseUrl}
              placeholder={withPassword ? 'https://hangar.example.com' : 'http://192.168.x.x:8000'}
              spellCheck={false}
              keyboardType="url"
              textContentType="URL"
            />

            {withPassword ? (
              <>
                <LoginField
                  label={m.login_usuario()}
                  value={user}
                  onChangeText={setUser}
                  placeholder={m.mobile_sync_login_user_hint()}
                  spellCheck={false}
                  textContentType="username"
                />
                <LoginField
                  label={m.login_senha()}
                  value={password}
                  onChangeText={setPassword}
                  placeholder="••••••••••••••••"
                  secureTextEntry
                  textContentType="password"
                />
              </>
            ) : (
              <LoginField
                label={m.sessao_token()}
                value={token}
                onChangeText={setToken}
                placeholder="••••••••••••••••"
                secureTextEntry
                textContentType="password"
              />
            )}

            {error ? (
              <View style={styles.errorBox}>
                <Text style={styles.errorText}>{error}</Text>
              </View>
            ) : null}

            <LoginSubmit label={submitLabel} text={loading ? m.login_conectando() : submitLabel} blocked={blocked} onPress={withPassword ? signIn : connect} />
            {loading && withPassword ? <Text accessibilityRole="alert" style={styles.waitText}>{m.mobile_sync_login_wait()}</Text> : null}

            <Pressable
              style={styles.scanBtn}
              onPress={() => { setWithPassword(!withPassword); setError(''); }}
              disabled={loading}
              accessibilityRole="button"
              accessibilityLabel={withPassword ? m.mobile_sync_login_token() : m.mobile_sync_login_open()}
            >
              <Text style={styles.scanText}>{withPassword ? m.mobile_sync_login_token() : m.mobile_sync_login_open()}</Text>
            </Pressable>
            <Pressable style={styles.scanBtn} onPress={() => setScanning(true)} disabled={loading} accessibilityLabel={m.login_qr()}>
              <Text style={styles.scanText}>{m.login_qr()}</Text>
            </Pressable>
            <Pressable style={styles.scanBtn} onPress={() => { setInviteLink(''); setInviteOpen(true); }} disabled={loading} accessibilityRole="button" accessibilityLabel={m.mobile_share_invite_open()}>
              <Text style={styles.scanText}>{m.mobile_share_invite_open()}</Text>
            </Pressable>
          </View>
        </Glass>
      </KeyboardAwareScrollView>

      {/* `scrollable` é o que faz a sheet dar a altura do detent ao conteúdo; sem ele o flex:1 da câmera colapsa numa faixa. */}
      <Sheet open={scanning} sizes={['large']} scrollable onDismiss={() => setScanning(false)}>
        <QrScanner onScan={handleScan} onClose={() => setScanning(false)} />
      </Sheet>
      <Sheet open={inviteOpen} sizes={['auto', 'large']} scrollable onDismiss={() => setInviteOpen(false)}>
        {inviteOpen ? <View style={styles.inviteForm}>
          <InvitePanel key={inviteLink} initialLink={inviteLink}
            onAccepted={(server, session) => router.replace(`/s/${encodeURIComponent(server.id)}/${encodeURIComponent(session)}`)} />
          <Pressable style={styles.scanBtn} onPress={() => setInviteOpen(false)} accessibilityRole="button" accessibilityLabel={m.comum_cancelar()}>
            <Text style={styles.scanText}>{m.comum_cancelar()}</Text>
          </Pressable>
        </View> : null}
      </Sheet>
    </Screen>
  );
}

const styles = StyleSheet.create((theme, rt) => ({
  container: {
    flexGrow: 1,
    justifyContent: 'flex-start',
    alignItems: 'center',
    paddingTop: 80,
    paddingHorizontal: theme.base.space[6],
    paddingBottom: theme.base.space[6],
  },
  inviteForm: { padding: theme.base.space[4], gap: theme.base.space[3] },
  card: {
    width: '100%',
    maxWidth: 400,
    padding: theme.base.space[6],
    gap: theme.base.space[5],
  },
  header: {
    alignItems: 'center',
    gap: theme.base.space[2],
    marginBottom: theme.base.space[2],
  },
  title: {
    fontSize: theme.base.text.xl,
    fontWeight: theme.base.weight.semibold,
    color: theme.tokens.text.primary,
    textAlign: 'center',
  },
  tagline: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.muted,
    textAlign: 'center',
  },
  form: {
    gap: theme.base.space[5],
  },
  errorBox: {
    backgroundColor: theme.tokens.status.error + '14',
    borderWidth: 1,
    borderColor: theme.tokens.status.error + '33',
    borderRadius: theme.base.radius.sm,
    padding: theme.base.space[3],
  },
  errorText: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.status.error,
  },
  waitText: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.muted,
    textAlign: 'center',
  },
  scanBtn: {
    height: 48,
    backgroundColor: 'transparent',
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    justifyContent: 'center',
    alignItems: 'center',
    minHeight: 44,
  },
  scanText: {
    color: theme.tokens.text.secondary,
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.medium,
  },
}));

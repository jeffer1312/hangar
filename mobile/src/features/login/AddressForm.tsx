import { useRef, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { LoginField, LoginSubmit } from './LoginFields';
import { messageStyles } from './messageStyles';
import { parsePairing } from './pairing';
import { signInAtAddress } from './addressSignIn';
import { useServers } from '../../stores/servers';
import { tailscaleFailure } from '../machines/api';
import { verifyLoginMachine } from '../machines/loginMachine';
import * as m from '../../paraglide/messages';

/** Endereço e token, ou endereço com usuário e senha de sincronização: o caminho de quem já sabe a máquina. */
export function AddressForm({ base: initialBase = '', token: initialToken = '' }: { base?: string; token?: string }) {
  const router = useRouter();
  const [baseUrl, setBaseUrl] = useState(initialBase);
  const [token, setToken] = useState(initialToken);
  const [withPassword, setWithPassword] = useState(false);
  const [user, setUser] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  // `loading` só chega no próximo render: dois toques seguidos entrariam duas vezes.
  const inFlight = useRef(false);

  const signIn = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    setError('');
    try {
      const servers = await signInAtAddress(baseUrl.trim(), user.trim(), password);
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
      inFlight.current = false;
      setLoading(false);
    }
  };

  const connect = async () => {
    if (inFlight.current) return;
    const base = baseUrl.trim();
    const tok = token.trim();
    // Monta texto de pareamento e valida pelo mesmo parsePairing do QR (regra única)
    const cru = base + (base.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(tok);
    const p = parsePairing(cru);
    if (!p) {
      setError(!base || !tok ? m.login_informe_url_token() : m.login_url_token_invalidos());
      return;
    }
    inFlight.current = true;
    setLoading(true);
    setError('');
    // add é síncrono em memória, mas persist é async: o probe só roda depois
    const before = useServers.getState();
    const existing = before.servers.find((s) => s.baseUrl === p.base && !s.invite);
    const added = before.add({ baseUrl: p.base, token: p.token, label: m.native_settings_group_server() });
    try {
      await verifyLoginMachine(added, !existing);
      router.replace('/');
    } catch (e) {
      // rollback: máquina já salva volta como estava; só o cadastro novo sai (mesmo que persist falhe, remove da memória)
      if (existing) {
        useServers.getState().add({ baseUrl: existing.baseUrl, token: existing.token });
        if (before.activeId) useServers.getState().setActive(before.activeId);
      } else {
        useServers.getState().remove(added.id);
      }
      const status = (e as { status?: number })?.status;
      const tailscale = tailscaleFailure(e, added.baseUrl);
      if (status === 401) setError(m.login_url_token_invalidos());
      else if (tailscale) setError(tailscale);
      else {
        const msg = e instanceof Error && e.message ? e.message : m.login_falha();
        setError(msg ? `${m.login_falha()}: ${msg}` : m.login_url_token_invalidos());
      }
    } finally {
      inFlight.current = false;
      setLoading(false);
    }
  };

  const blocked = loading || (withPassword ? !baseUrl.trim() || !user.trim() || !password : !token.trim());
  const submitLabel = withPassword ? m.login_entrar() : m.login_conectar();
  const toggleLabel = withPassword ? m.mobile_sync_login_token() : m.mobile_sync_login_open();
  return <View style={styles.form}>
    <LoginField label={m.sessao_url_servidor()} value={baseUrl} onChangeText={setBaseUrl}
      placeholder={withPassword ? 'https://hangar.example.com' : 'http://192.168.x.x:8000'} spellCheck={false} keyboardType="url" textContentType="URL" />
    {withPassword ? <>
      <LoginField label={m.login_usuario()} value={user} onChangeText={setUser} placeholder={m.mobile_sync_login_user_hint()}
        spellCheck={false} textContentType="username" />
      <LoginField label={m.login_senha()} value={password} onChangeText={setPassword} placeholder="••••••••••••••••" secureTextEntry textContentType="password" />
    </> : <LoginField label={m.sessao_token()} value={token} onChangeText={setToken} placeholder="••••••••••••••••" secureTextEntry textContentType="password" />}
    {error ? <View style={messageStyles.errorBox}><Text accessibilityRole="alert" style={messageStyles.errorText}>{error}</Text></View> : null}
    <LoginSubmit label={submitLabel} text={loading ? m.login_conectando() : submitLabel} blocked={blocked} onPress={withPassword ? signIn : connect} />
    {loading && withPassword ? <Text accessibilityRole="alert" style={messageStyles.waitText}>{m.mobile_sync_login_wait()}</Text> : null}
    <Pressable style={styles.secondary} onPress={() => { setWithPassword(!withPassword); setError(''); }} disabled={loading}
      accessibilityRole="button" accessibilityLabel={toggleLabel}>
      <Text style={styles.secondaryText}>{toggleLabel}</Text>
    </Pressable>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  form: { gap: theme.base.space[4], paddingTop: theme.base.space[2] },
  secondary: { minHeight: 44, justifyContent: 'center', alignItems: 'center' },
  secondaryText: { color: theme.tokens.text.secondary, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
}));

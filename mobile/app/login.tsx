import { useState } from 'react';
import { Pressable, Text, TextInput, View, Modal } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Screen } from '../src/ui/Screen';
import { Glass } from '../src/ui/Glass';
import { QrScanner } from '../src/features/login/QrScanner';
import { parsePairing } from '../src/features/login/pairing';
import { useServers } from '../src/stores/servers';
import { getSessions } from '@hangar/core';
import * as m from '../src/paraglide/messages';

export default function Login() {
  const router = useRouter();
  const [baseUrl, setBaseUrl] = useState('');
  const [token, setToken] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [scanning, setScanning] = useState(false);

  const handleScan = (texto: string) => {
    const cru = texto.trim();
    const p = parsePairing(cru);
    setScanning(false);
    if (!p) {
      setError(cru.includes('://') ? m.login_url_token_invalidos() : m.login_qr_sem_url());
      return;
    }
    setBaseUrl(p.base);
    setToken(p.token);
    setError('');
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
    const added = useServers.getState().add({ baseUrl: p.base, token: p.token });

    try {
      await getSessions();
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

  return (
    <Screen>
      <View style={styles.container}>
        <Glass variant="modal" style={styles.card}>
          <View style={styles.header}>
            <Text style={styles.title}>{m.login_titulo()}</Text>
            <Text style={styles.tagline}>{m.login_tagline()}</Text>
          </View>

          <View style={styles.form}>
            <View style={styles.field}>
              <Text style={styles.label}>{m.sessao_url_servidor()}</Text>
              <TextInput
                style={styles.input}
                value={baseUrl}
                onChangeText={setBaseUrl}
                placeholder="http://192.168.x.x:8000"
                placeholderTextColor="#8d8489"
                autoCapitalize="none"
                autoCorrect={false}
                spellCheck={false}
                keyboardType="url"
                textContentType="URL"
                accessibilityLabel={m.sessao_url_servidor()}
              />
            </View>

            <View style={styles.field}>
              <Text style={styles.label}>{m.sessao_token()}</Text>
              <TextInput
                style={styles.input}
                value={token}
                onChangeText={setToken}
                placeholder="••••••••••••••••"
                placeholderTextColor="#8d8489"
                secureTextEntry
                autoCapitalize="none"
                autoCorrect={false}
                textContentType="password"
                accessibilityLabel={m.sessao_token()}
              />
            </View>

            {error ? (
              <View style={styles.errorBox}>
                <Text style={styles.errorText}>{error}</Text>
              </View>
            ) : null}

            <Pressable
              style={[styles.connectBtn, (loading || !token.trim()) && styles.connectBtnDisabled]}
              onPress={connect}
              disabled={loading || !token.trim()}
              accessibilityLabel={m.login_conectar()}
              accessibilityState={{ disabled: loading || !token.trim() }}
            >
              <Text style={[styles.connectText, (loading || !token.trim()) && styles.connectTextDisabled]}>
                {loading ? m.login_conectando() : m.login_conectar()}
              </Text>
            </Pressable>

            <Pressable style={styles.scanBtn} onPress={() => setScanning(true)} accessibilityLabel={m.login_qr()}>
              <Text style={styles.scanText}>{m.login_qr()}</Text>
            </Pressable>
          </View>
        </Glass>
      </View>

      <Modal visible={scanning} animationType="slide" onRequestClose={() => setScanning(false)}>
        <QrScanner onScan={handleScan} onClose={() => setScanning(false)} />
      </Modal>
    </Screen>
  );
}

const styles = StyleSheet.create((theme, rt) => ({
  container: {
    flex: 1,
    justifyContent: 'flex-start',
    alignItems: 'center',
    paddingTop: 80,
    paddingHorizontal: theme.base.space[6],
    paddingBottom: theme.base.space[6],
  },
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
  field: {
    gap: theme.base.space[2],
  },
  label: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.medium,
    color: theme.tokens.text.secondary,
  },
  input: {
    height: 48,
    backgroundColor: theme.tokens.bg.elevated,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    color: theme.tokens.text.primary,
    fontSize: 16,
    paddingHorizontal: theme.base.space[4],
  },
  errorBox: {
    backgroundColor: 'rgba(255,69,58,0.08)',
    borderWidth: 1,
    borderColor: 'rgba(255,69,58,0.2)',
    borderRadius: theme.base.radius.sm,
    padding: theme.base.space[3],
  },
  errorText: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.status.error,
  },
  connectBtn: {
    height: 52,
    backgroundColor: theme.tokens.accent.base,
    borderRadius: theme.base.radius.md,
    justifyContent: 'center',
    alignItems: 'center',
    minHeight: 44,
  },
  connectBtnDisabled: {
    backgroundColor: theme.tokens.bg.hover,
  },
  connectText: {
    color: '#fff',
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
  },
  connectTextDisabled: {
    color: theme.tokens.text.muted,
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

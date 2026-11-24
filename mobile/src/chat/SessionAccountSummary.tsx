import { useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { cotaDoModeloAgy, getCredentialsForServer, resumoCota, type Credencial, type Server } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { superficie } from '../theme/superficie';
import { sessionCredential, type AccountSession } from './sessionAccountOptions';
import * as m from '../paraglide/messages';

export function SessionAccountSummary({ server, session, label, onPress }: {
  server: Server; session: AccountSession; label: string; onPress?: () => void;
}) {
  const { theme } = useUnistyles();
  const [state, setState] = useState<{ loading: boolean; credential?: Credencial; error?: string }>({ loading: true });
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let active = true;
    setState({ loading: true });
    const request = session.engine ? getCredentialsForServer(server, false, undefined, true) : getCredentialsForServer(server);
    void request.then(credentials => {
      if (active) setState({ loading: false, credential: sessionCredential(session, credentials) });
    }).catch(error => {
      if (active) setState({ loading: false, error: error instanceof Error ? error.message : m.native_invalid_response() });
    });
    return () => { active = false; };
  }, [server, session.conta, session.config_dir, session.codex_home, session.codex_account, session.engine, session.engine_account, session.engine_binding, retry]);
  const credential = state.credential;
  const noBinding = credential?.cliproxy && !session.engine_account;
  // No Antigravity a sessão vê só o grupo de modelos dela (Gemini ou Claude/GPT).
  const cota = credential?.cota && credential.tipo === 'antigravity' ? cotaDoModeloAgy(credential.cota, session.model) : credential?.cota;
  const quota = !noBinding && cota ? resumoCota({ ...cota, id: credential!.id }) : '';
  const accountName = noBinding ? m.cliproxy_no_bound_account() : credential?.apelido || credential?.nome || label;
  const detail = state.loading ? m.comum_carregando() : state.error ? m.conta_erro_listar({ erro: state.error }) : quota || m.conta_cota_sem_leitura();
  const content = <>
    <Icon name="CircleUser" size={22} color={theme.tokens.accent.base} />
    <View style={styles.body}>
      <Text style={styles.name}>{accountName}</Text>
      {state.loading ? <ActivityIndicator size="small" color={theme.tokens.text.muted} />
        : <Text style={styles.quota}>{detail}</Text>}
    </View>
    {onPress ? <Icon name="ChevronRight" size={18} color={theme.tokens.text.muted} /> : null}
  </>;
  return <View>
    {onPress ? <Pressable accessibilityRole="button" accessibilityLabel={m.accounts_switch_title()} accessibilityValue={{ text: `${accountName}, ${detail}` }} onPress={onPress}
      style={({ pressed }) => [styles.row, { backgroundColor: superficie(theme, 0.4) }, pressed && styles.pressed]}>{content}</Pressable>
      : <View style={styles.row}>{content}</View>}
    {state.error ? <Pressable accessibilityRole="button" accessibilityLabel={m.native_retry()} onPress={() => setRetry(n => n + 1)} style={styles.retry}>
      <Text style={styles.quota}>{m.native_retry()}</Text>
    </Pressable> : null}
  </View>;
}
const styles = StyleSheet.create(theme => ({
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], borderRadius: theme.base.radius.lg, padding: theme.base.space[3], minHeight: 44, marginHorizontal: theme.base.space[1], marginVertical: theme.base.space[1] },
  body: { flex: 1, minWidth: 0, gap: theme.base.space[1] },
  name: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  quota: { fontSize: theme.base.text.xxs, color: theme.tokens.text.secondary },
  pressed: { opacity: 0.7 },
  retry: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[4] },
}));

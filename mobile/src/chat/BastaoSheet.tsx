import { useCallback, useEffect, useMemo, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { getBastaoDossie } from '@hangar/core';
import { mkMarkdownStyle } from './AssistantBubble';
import * as m from '../paraglide/messages';

// O dossiê que ESTA sessão recebeu. 404 é resposta, não falha: a sessão simplesmente não veio de
// uma passagem de bastão.
export function BastaoSheet({ name }: { name: string }) {
  const { theme } = useUnistyles();
  const md = useMemo(() => mkMarkdownStyle(theme), [theme]);
  const [texto, setTexto] = useState<string | null>(null);
  const [vazio, setVazio] = useState(false);
  const [erro, setErro] = useState('');
  const [carregando, setCarregando] = useState(true);
  const [tentativa, setTentativa] = useState(0);

  useEffect(() => {
    let vivo = true;
    setCarregando(true);
    setErro('');
    setVazio(false);
    getBastaoDossie(name)
      .then((t) => {
        if (vivo) setTexto(t);
      })
      .catch((e: unknown) => {
        if (!vivo) return;
        if ((e as { status?: number } | null)?.status === 404) setVazio(true);
        else setErro(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (vivo) setCarregando(false);
      });
    return () => {
      vivo = false;
    };
  }, [name, tentativa]);

  const tentarDeNovo = useCallback(() => setTentativa((n) => n + 1), []);

  return (
    <View style={[styles.container, { backgroundColor: theme.tokens.bg.base }]}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.bastao_dossie_titulo()}</Text>
      {carregando ? (
        <View style={styles.center}>
          <ActivityIndicator color={theme.tokens.text.secondary} />
          <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
        </View>
      ) : vazio ? (
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.bastao_vazio()}</Text>
      ) : erro ? (
        <View style={styles.center}>
          <Text style={[styles.err, { color: theme.tokens.status.error }]}>{erro}</Text>
          <Pressable onPress={tentarDeNovo} accessibilityRole="button" style={styles.retry}>
            <Text style={[styles.retryTxt, { color: theme.tokens.accent.base }]}>{m.lista_tentar_novamente()}</Text>
          </Pressable>
        </View>
      ) : (
        <ScrollView contentContainerStyle={styles.corpo}>
          <EnrichedMarkdownText markdown={texto ?? ''} markdownStyle={md} flavor="github" />
        </ScrollView>
      )}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  container: { flex: 1, padding: theme.base.space[4], gap: theme.base.space[3] },
  title: { fontSize: theme.base.text.lg, fontWeight: '600' },
  center: { alignItems: 'center', gap: theme.base.space[2], paddingVertical: theme.base.space[4] },
  muted: { fontSize: theme.base.text.sm, textAlign: 'center' },
  err: { fontSize: theme.base.text.sm, textAlign: 'center' },
  retry: { minHeight: 44, justifyContent: 'center' },
  retryTxt: { fontSize: theme.base.text.sm, fontWeight: '600' },
  corpo: { paddingBottom: theme.base.space[4] },
}));

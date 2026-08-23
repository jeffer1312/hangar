import { useEffect, useRef, useState } from 'react';
import { Pressable, Text, TextInput, View, ScrollView } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { FileSearchHit } from '@hangar/core';
import * as m from '../../paraglide/messages';

type ModoBusca = 'names' | 'contents';

interface Props {
  q: string;
  mode: ModoBusca;
  resultados: FileSearchHit[];
  buscaCortada: boolean;
  onBusca: (q: string, mode: ModoBusca) => void;
  onPick: (path: string) => void;
}

export function FileSearchBar({ q, mode, resultados, buscaCortada, onBusca, onPick }: Props) {
  const { theme } = useUnistyles();
  const [texto, setTexto] = useState(q);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // sincroniza prop q (troca de sessão)
  useEffect(() => setTexto(q), [q]);

  function agendar(novo: string, m: ModoBusca) {
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => onBusca(novo, m), 300);
  }

  function handleChange(t: string) {
    setTexto(t);
    agendar(t, mode);
  }

  function escolherModo(novo: ModoBusca) {
    if (novo === mode) return;
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    onBusca(texto, novo);
  }

  const temBusca = q.trim() !== '';
  const vazio = temBusca && resultados.length === 0;

  return (
    <View style={styles.root}>
      <View style={[styles.campo, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.subtle }]}>
        <Text style={styles.lupa}>🔍</Text>
        <TextInput
          value={texto}
          onChangeText={handleChange}
          placeholder={m.arq_buscar()}
          placeholderTextColor={theme.tokens.text.muted}
          style={[styles.input, { color: theme.tokens.text.primary }]}
          accessibilityLabel={m.arq_buscar()}
        />
      </View>
      <View style={styles.seg}>
        <Pressable
          onPress={() => escolherModo('names')}
          style={[styles.segBtn, mode === 'names' ? { backgroundColor: theme.tokens.accent.dim } : null]}
        >
          <Text style={[styles.segTxt, { color: mode === 'names' ? theme.tokens.accent.base : theme.tokens.text.secondary }]}>{m.arq_modo_nomes()}</Text>
        </Pressable>
        <Pressable
          onPress={() => escolherModo('contents')}
          style={[styles.segBtn, mode === 'contents' ? { backgroundColor: theme.tokens.accent.dim } : null]}
        >
          <Text style={[styles.segTxt, { color: mode === 'contents' ? theme.tokens.accent.base : theme.tokens.text.secondary }]}>{m.arq_modo_conteudo()}</Text>
        </Pressable>
      </View>

      {temBusca ? (
        <View style={styles.resultArea}>
          {buscaCortada ? <Text style={[styles.aviso, { color: theme.tokens.text.muted }]}>{m.arq_primeiros_200()}</Text> : null}
          {vazio ? (
            <Text style={[styles.aviso, { color: theme.tokens.text.muted }]}>{m.arq_sem_resultados()}</Text>
          ) : (
            <ScrollView>
              {resultados.map((hit) => {
                const nome = hit.path.slice(hit.path.lastIndexOf('/') + 1);
                return (
                  <Pressable key={hit.path + String(hit.line)} onPress={() => onPick(hit.path)} style={styles.hit}>
                    <Text style={[styles.hitNome, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                      {nome}
                    </Text>
                    <Text style={[styles.hitPath, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                      {hit.path}
                    </Text>
                    {mode === 'contents' && hit.line !== null && hit.text !== null ? (
                      <Text style={[styles.hitTrecho, { color: theme.tokens.text.secondary }]} numberOfLines={1}>
                        {hit.line}: {hit.text}
                      </Text>
                    ) : null}
                  </Pressable>
                );
              })}
            </ScrollView>
          )}
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
  },
  campo: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 8,
    paddingVertical: 6,
    marginHorizontal: theme.base.space[3],
    marginBottom: 6,
  },
  lupa: { fontSize: 12 },
  input: {
    flex: 1,
    fontSize: 14,
    padding: 0,
  },
  seg: {
    flexDirection: 'row',
    gap: 6,
    marginHorizontal: theme.base.space[3],
    marginBottom: 8,
  },
  segBtn: {
    flex: 1,
    paddingVertical: 8,
    borderRadius: 8,
    alignItems: 'center',
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
  },
  segTxt: {
    fontSize: 12,
    fontWeight: '500',
  },
  resultArea: {
    flex: 1,
  },
  aviso: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 4,
  },
  hit: {
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 8,
    borderBottomWidth: 1,
    borderBottomColor: theme.tokens.border.subtle,
    minHeight: 44,
  },
  hitNome: {
    fontSize: 14,
    fontWeight: '500',
  },
  hitPath: {
    fontSize: 11,
  },
  hitTrecho: {
    fontSize: 12,
    fontFamily: theme.base.fontMono,
  },
}));

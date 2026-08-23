import { Pressable, Text, View, ScrollView } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { TreeEntry } from '@hangar/core';
import * as m from '../../paraglide/messages';

interface Props {
  entries: TreeEntry[];
  abertos: Set<string>;
  selecionado: string | null;
  onToggle: (path: string) => void;
  onPick: (path: string) => void;
  listaCortada?: boolean;
  soModificados?: boolean;
  onToggleFiltro?: () => void;
}

const RECUO = 14;
const BASE = 8;

function nivel(path: string) {
  return path.split('/').length - 1;
}

function corMarca(c: TreeEntry['changed'], tokens: { status: { warning: string; success: string; error: string }; text: { muted: string } }) {
  if (c === 'A') return tokens.status.success;
  if (c === 'D') return tokens.status.error;
  if (c === '?') return tokens.text.muted;
  return tokens.status.warning;
}

export function FileTree({ entries, abertos, selecionado, onToggle, onPick, listaCortada, soModificados, onToggleFiltro }: Props) {
  const { theme } = useUnistyles();

  return (
    <View style={styles.root}>
      {listaCortada ? <Text style={[styles.aviso, { color: theme.tokens.text.muted }]}>{m.arq_pasta_grande()}</Text> : null}
      {entries.length === 0 && soModificados ? <Text style={[styles.aviso, { color: theme.tokens.text.muted }]}>{m.arq_nada_mudou()}</Text> : null}
      <ScrollView contentContainerStyle={styles.scroll} showsVerticalScrollIndicator={false}>
        {entries.map((ent) => {
          const isSel = selecionado === ent.path;
          const pad = BASE + nivel(ent.path) * RECUO;
          const isDir = ent.is_dir;
          const aberta = abertos.has(ent.path);
          return (
            <Pressable
              key={ent.path}
              onPress={() => (isDir ? onToggle(ent.path) : onPick(ent.path))}
              style={[
                styles.row,
                {
                  paddingLeft: pad,
                  backgroundColor: isSel ? theme.tokens.accent.dim : 'transparent',
                },
              ]}
              accessibilityRole="button"
            >
              <Text style={[styles.chev, { color: theme.tokens.text.muted }]}>{isDir ? (aberta ? '▾' : '▸') : ''}</Text>
              <Text style={[styles.icon, { color: theme.tokens.text.muted }]}>{isDir ? '📁' : '📄'}</Text>
              <Text style={[styles.nome, { color: isDir ? theme.tokens.text.primary : theme.tokens.text.secondary }]} numberOfLines={1}>
                {ent.name}
              </Text>
              {ent.add || ent.del ? (
                <View style={styles.numView}>
                  <Text style={[styles.numAdd, { color: theme.tokens.status.success }]}>+{ent.add}</Text>
                  <Text style={[styles.numDel, { color: theme.tokens.status.error }]}> −{ent.del}</Text>
                </View>
              ) : null}
              {ent.changed ? (
                <Text style={[styles.marca, { color: corMarca(ent.changed, theme.tokens) }]}>{ent.changed}</Text>
              ) : null}
            </Pressable>
          );
        })}
      </ScrollView>
      {onToggleFiltro ? (
        <Pressable onPress={onToggleFiltro} style={styles.filtroBtn} accessibilityRole="button">
          <Text style={[styles.filtroTxt, { color: theme.tokens.accent.base }]}>
            {soModificados ? m.arq_mostrar_tudo() : m.arq_mostrar_so_modificados()}
          </Text>
        </Pressable>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
  },
  aviso: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[1],
  },
  scroll: {
    paddingVertical: theme.base.space[1],
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    paddingVertical: 4,
    paddingRight: theme.base.space[3],
    minHeight: 44,
  },
  chev: {
    width: 12,
    textAlign: 'center',
    fontSize: 9,
  },
  icon: {
    width: 14,
    textAlign: 'center',
    fontSize: 11,
  },
  nome: {
    flex: 1,
    fontSize: 13,
  },
  numView: {
    flexDirection: 'row',
  },
  numAdd: {
    fontSize: 10.5,
    fontFamily: theme.base.fontMono,
  },
  numDel: {
    fontSize: 10.5,
    fontFamily: theme.base.fontMono,
  },
  marca: {
    width: 14,
    textAlign: 'center',
    fontSize: 11,
    fontWeight: '600',
    fontFamily: theme.base.fontMono,
  },
  filtroBtn: {
    padding: theme.base.space[2],
    alignItems: 'center',
  },
  filtroTxt: {
    fontSize: theme.base.text.xs,
    fontWeight: '500',
  },
}));

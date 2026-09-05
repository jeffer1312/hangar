import { ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { computeEditDiff, type SplitRow } from '@hangar/core';

// SplitRow não carrega o op da linha: quem diz o que ela é são os dois lados juntos — mesmos textos
// = contexto, lado vazio ou textos diferentes = removida à esquerda, adicionada à direita. Mesma
// regra do EditDiff.svelte.
function ladoDe(row: SplitRow, lado: 'left' | 'right'): 'ctx' | 'del' | 'add' | null {
  if (!row[lado]) return null;
  if (row.left && row.right && row.left.text === row.right.text) return 'ctx';
  return lado === 'left' ? 'del' : 'add';
}

// Diff do Edit/Write sobre o computeEditDiff do core (a mesma conta da PWA). Sem flag de
// truncamento: acima do teto de Myers o core já devolve del+add em bloco.
export function EditDiff({ oldText, newText, modo = 'unificado' }: { oldText: string; newText: string; modo?: 'unificado' | 'lado' }) {
  const { theme } = useUnistyles();
  const d = computeEditDiff(oldText, newText);
  const bg = (op: 'ctx' | 'del' | 'add' | null) => op === 'add' ? theme.tokens.status.success + '2e' : op === 'del' ? theme.tokens.status.error + '2e' : 'transparent';
  if (modo === 'lado') {
    return (
      <ScrollView horizontal>
        <View>
          {d.rows.map((r, i) => (
            <View key={i} style={styles.row}>
              <Text style={[styles.cell, { color: theme.tokens.text.primary, backgroundColor: bg(ladoDe(r, 'left')) }]}>{r.left ? `${r.left.num} ${r.left.text}` : ''}</Text>
              <Text style={[styles.cell, { color: theme.tokens.text.primary, backgroundColor: bg(ladoDe(r, 'right')) }]}>{r.right ? `${r.right.num} ${r.right.text}` : ''}</Text>
            </View>
          ))}
        </View>
      </ScrollView>
    );
  }
  return (
    <View>
      {d.ops.map((l, i) => (
        <Text key={i} style={[styles.line, { backgroundColor: bg(l.op), color: theme.tokens.text.primary }]}>
          {l.op === 'add' ? '+' : l.op === 'del' ? '−' : ' '} {l.text}
        </Text>
      ))}
      <Text style={[styles.line, { color: theme.tokens.text.muted }]}>+{d.add} −{d.del}</Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  line: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, paddingHorizontal: 6, paddingVertical: 1 },
  row: { flexDirection: 'row' },
  cell: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, minWidth: 180, paddingHorizontal: 6 },
}));

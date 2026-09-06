import { useMemo, useState } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { computeEditDiff, type SplitRow } from '@hangar/core';
import * as m from '../../paraglide/messages';

// Cada linha do diff é um <Text> próprio, sem virtualização: um Write de arquivo inteiro monta
// milhares de views de uma vez. Mostra o começo e deixa o resto atrás de um toque.
const TETO = 300;

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
  const [tudo, setTudo] = useState(false);
  // Myers sobre arquivo inteiro não pode rodar de novo a cada render do tema.
  const d = useMemo(() => computeEditDiff(oldText, newText), [oldText, newText]);
  const bg = (op: 'ctx' | 'del' | 'add' | null) => op === 'add' ? theme.tokens.status.success + '2e' : op === 'del' ? theme.tokens.status.error + '2e' : 'transparent';
  const total = modo === 'lado' ? d.rows.length : d.ops.length;
  const corte = tudo ? total : Math.min(TETO, total);
  const maisLinhas = corte < total ? (
    <Pressable onPress={() => setTudo(true)} style={styles.mais} accessibilityRole="button">
      <Text style={[styles.maisTxt, { color: theme.tokens.accent.base }]}>{m.tool_mostrar_mais_linhas({ n: total - corte })}</Text>
    </Pressable>
  ) : null;
  if (modo === 'lado') {
    return (
      <ScrollView horizontal>
        <View>
          {d.rows.slice(0, corte).map((r, i) => (
            <View key={i} style={styles.row}>
              <Text style={[styles.cell, { color: theme.tokens.text.primary, backgroundColor: bg(ladoDe(r, 'left')) }]}>{r.left ? `${r.left.num} ${r.left.text}` : ''}</Text>
              <Text style={[styles.cell, { color: theme.tokens.text.primary, backgroundColor: bg(ladoDe(r, 'right')) }]}>{r.right ? `${r.right.num} ${r.right.text}` : ''}</Text>
            </View>
          ))}
          {maisLinhas}
        </View>
      </ScrollView>
    );
  }
  return (
    <View>
      {d.ops.slice(0, corte).map((l, i) => (
        <Text key={i} style={[styles.line, { backgroundColor: bg(l.op), color: theme.tokens.text.primary }]}>
          {l.op === 'add' ? '+' : l.op === 'del' ? '−' : ' '} {l.text}
        </Text>
      ))}
      {maisLinhas}
      <Text style={[styles.line, { color: theme.tokens.text.muted }]}>+{d.add} −{d.del}</Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  line: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, paddingHorizontal: 6, paddingVertical: 1 },
  row: { flexDirection: 'row' },
  cell: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, minWidth: 180, paddingHorizontal: 6 },
  mais: { paddingVertical: 8, paddingHorizontal: 6, minHeight: 44, justifyContent: 'center' },
  maisTxt: { fontSize: theme.base.text.xs, fontWeight: '600' },
}));

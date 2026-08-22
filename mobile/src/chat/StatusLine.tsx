import { useMemo } from 'react';
import { ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { parseStatusLine } from '@hangar/core';
import { ContextRing } from './ContextRing';
import { statusChips } from './statusChips';

// Statusline parseada (parseStatusLine do core): anel de contexto + modelo/effort + chips
// (custo, janelas de cota, repo/branch, tempo). Sessão sem marcador nenhum segue mostrando a
// linha crua — nunca fica sem linha. O numberOfLines={1} do P1 sumiu: a fileira rola na
// horizontal, nada é truncado ("status line truncada" registrado no Plano 1).
export function StatusLine({ line }: { line: string | null }) {
  const { theme } = useUnistyles();
  const f = useMemo(() => parseStatusLine(line), [line]);
  const chips = useMemo(() => statusChips(f), [f]);
  if (!line) return null;
  // parseStatusLine só devolve null com line vazia; "não parseou" aqui é nenhum marcador
  // reconhecido (objeto com só o raw) — nesse caso, cru como antes.
  const parseou = !!f && (!!f.model || !!f.branch || f.ctxPct != null || chips.length > 0);
  if (!parseou) {
    return (
      <View style={styles.wrap}>
        <ScrollView horizontal showsHorizontalScrollIndicator={false}>
          <Text style={[styles.txt, { color: theme.tokens.text.muted }]}>{line}</Text>
        </ScrollView>
      </View>
    );
  }
  return (
    <View style={styles.wrap}>
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.row}
        accessibilityLabel={line}
      >
        <ContextRing pct={f!.ctxPct ?? null} />
        {f!.model ? (
          <Text style={styles.pill} numberOfLines={1}>
            {`${f!.model}${f!.effort ? ` (${f!.effort})` : ''}`}
          </Text>
        ) : null}
        {chips.map((c) => (
          <Text key={c.key} style={[styles.pill, c.warn && styles.pillWarn]} numberOfLines={1}>
            {c.text}
          </Text>
        ))}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    alignSelf: 'stretch',
    paddingVertical: theme.base.space[1],
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingHorizontal: theme.base.space[3],
  },
  txt: {
    fontSize: theme.base.text.xxs,
    fontFamily: theme.base.fontMono,
    paddingHorizontal: theme.base.space[3],
  },
  pill: {
    fontSize: theme.base.text.xxs,
    fontFamily: theme.base.fontMono,
    color: theme.tokens.text.secondary,
    backgroundColor: theme.tokens.bg.surface,
    borderColor: theme.tokens.border.subtle,
    borderWidth: 1,
    borderRadius: 999,
    overflow: 'hidden',
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 2,
  },
  pillWarn: {
    color: theme.tokens.status.warning,
    borderColor: theme.tokens.status.warning,
  },
}));

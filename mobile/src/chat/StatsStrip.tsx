import { ScrollView, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { abbrevNum, type StatsEvent } from '@hangar/core';
import * as m from '../paraglide/messages';

const fmtDur = (ms: number) =>
  ms < 60_000 ? `${Math.round(ms / 1000)}s` : `${Math.floor(ms / 60_000)}m${Math.round((ms % 60_000) / 1000)}s`;

// Mesma ordem da faixa da PWA; campo que não veio não vira parte vazia.
export function linhaStats(s: StatsEvent): string[] {
  const p = [
    s.turns === 1 ? m.stats_turnos_1() : m.stats_turnos({ n: s.turns }),
    s.steps === 1 ? m.stats_chamadas_1() : m.stats_chamadas({ n: s.steps }),
  ];
  if (s.llm_ms) {
    p.push(m.stats_llm({ t: fmtDur(s.llm_ms) }));
    if (s.tool_ms) p.push(m.stats_tools({ t: fmtDur(s.tool_ms) }));
  }
  if (s.tok_s) p.push(m.stats_toks({ n: Math.round(s.tok_s) }));
  if (s.ttft_ms) p.push(m.stats_ttft({ t: fmtDur(s.ttft_ms) }));
  if (s.cache_pct != null) p.push(m.stats_cache({ n: s.cache_pct }));
  p.push(m.stats_io({ i: abbrevNum(s.in_tok), o: abbrevNum(s.out_tok) }));
  return p;
}

export function StatsStrip({ stats }: { stats: StatsEvent | null }) {
  const { theme } = useUnistyles();
  if (!stats) return null;
  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      // Sem o flexGrow: 0 a ScrollView come toda a altura que sobra da coluna e empurra a lista
      // pra fora da tela — ela só precisa da altura da própria linha.
      style={styles.fora}
      contentContainerStyle={styles.strip}
      accessibilityLabel={m.stats_faixa_aria()}
    >
      <Text style={[styles.txt, { color: theme.tokens.text.muted }]}>{linhaStats(stats).join('  ·  ')}</Text>
    </ScrollView>
  );
}

const styles = StyleSheet.create(() => ({
  fora: { flexGrow: 0 },
  strip: { paddingHorizontal: 12, paddingVertical: 4 },
  txt: { fontSize: 11 },
}));

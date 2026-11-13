import { View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../../ui/Icon';
import type { Ferramentas, Leitura } from '../../stores/aparencia';
import { useSettingsColors } from './colors';

// Miniaturas da Aparência que não são de tema nem de fundo: art_reading e art_tools do Rust.

const INK = '#ffffff';

/** Leitura: a mesma foto (duas manchas fixas); o que muda é o que segura o texto. */
export function ReadingArt({ leitura }: { leitura: Leitura }) {
  const linhas = (halo: boolean) => (
    <View style={styles.lines}>
      {['100%', '70%'].map((w) => (
        <View key={w} style={[{ width: w as `${number}%` }, halo && styles.halo]}>
          <View style={[styles.bar, { backgroundColor: INK, opacity: halo ? 1 : 0.8 }]} />
        </View>
      ))}
    </View>
  );
  return (
    <View style={styles.photo}>
      <View style={styles.spot} />
      {leitura === 'sheet' ? (
        <View style={styles.sheet}>
          <View style={[styles.bar, { width: '100%', backgroundColor: INK }]} />
          <View style={[styles.bar, { width: '70%', backgroundColor: INK }]} />
        </View>
      ) : (
        linhas(leitura === 'text')
      )}
      {/* Automática escolhe sozinha: o selo marca que é o app quem decide. */}
      {leitura === 'auto' ? (
        <View style={styles.seal}><Icon name="Sparkles" size={11} color={INK} /></View>
      ) : null}
    </View>
  );
}

/** Chamadas de ferramenta: linhas com marca (Clássico), pílulas (Chips), tronco (Árvore) ou resumo e cartão (Claude Desktop). */
export function ToolsArt({ look }: { look: Ferramentas }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const tom = c.faint;
  const bar = (w: `${number}%`) => <View style={[styles.bar, { width: w, backgroundColor: tom, opacity: 0.6 }]} />;
  if (look === 'terminal') {
    return (
      <View style={[styles.pad, styles.col5]}>
        <View style={styles.classicRow}>
          <View style={[styles.dot, { backgroundColor: theme.tokens.status.success }]} />
          <View style={styles.flex}>{bar('50%')}</View>
        </View>
        {[theme.tokens.status.error, theme.tokens.status.success].map((color) => (
          <View key={color} style={[styles.diff, { backgroundColor: color, opacity: 0.45 }]} />
        ))}
      </View>
    );
  }
  if (look === 'claudeDesktop') {
    return (
      <View style={[styles.pad, styles.col5]}>
        {bar('65%')}
        <View style={[styles.card, { borderColor: tom }]}>
          {(['70%', '50%'] as const).map((w, i) => (
            <View key={w} style={[styles.cardRow, i > 0 && { borderTopWidth: 1, borderTopColor: tom }]}>{bar(w)}</View>
          ))}
        </View>
      </View>
    );
  }
  if (look === 'chips') {
    return (
      <View style={[styles.pad, styles.wrap]}>
        {[40, 28, 48, 34].map((w, i) => (
          <View key={i} style={[styles.chip, { width: w, backgroundColor: tom, opacity: 0.35 }]} />
        ))}
      </View>
    );
  }
  if (look === 'tree') {
    return (
      <View style={[styles.pad, styles.col5]}>
        {bar('55%')}
        {(['60%', '45%', '70%'] as const).map((w) => (
          <View key={w} style={[styles.branch, { borderLeftColor: tom }]}>{bar(w)}</View>
        ))}
      </View>
    );
  }
  return (
    <View style={[styles.pad, styles.col6]}>
      {(['60%', '75%', '45%'] as const).map((w) => (
        <View key={w} style={styles.classicRow}>
          <View style={[styles.mark, { backgroundColor: tom, opacity: 0.6 }]} />
          <View style={styles.flex}>{bar(w)}</View>
        </View>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  photo: { flex: 1, backgroundColor: '#2a3b55', overflow: 'hidden' },
  spot: { position: 'absolute', right: -12, bottom: -14, width: 64, height: 40, borderRadius: 32, backgroundColor: '#3d4f3a' },
  lines: { position: 'absolute', left: 8, top: 12, right: 16, gap: 5 },
  halo: { padding: 2, borderRadius: 3, backgroundColor: 'rgba(0,0,0,0.55)' },
  bar: { height: 4, borderRadius: 2 },
  sheet: { position: 'absolute', left: 5, top: 6, right: 10, bottom: 6, borderRadius: 5, backgroundColor: 'rgba(0,0,0,0.72)', padding: 6, gap: 5 },
  seal: {
    position: 'absolute', top: 4, right: 4, width: 16, height: 16, borderRadius: 4,
    backgroundColor: 'rgba(0,0,0,0.6)', alignItems: 'center', justifyContent: 'center',
  },
  pad: { flex: 1, padding: 8 },
  wrap: { flexDirection: 'row', flexWrap: 'wrap', gap: 5 },
  chip: { height: 12, borderRadius: 6 },
  col5: { gap: 5 },
  col6: { gap: 6 },
  branch: { marginLeft: 3, paddingLeft: 6, borderLeftWidth: 1 },
  classicRow: { flexDirection: 'row', alignItems: 'center', gap: 5 },
  mark: { width: 7, height: 7, borderRadius: 2 },
  dot: { width: 6, height: 6, borderRadius: 3 },
  diff: { marginLeft: 11, height: 5, width: '70%', borderRadius: 2 },
  flex: { flex: 1 },
  card: { borderWidth: 1, borderRadius: 4, opacity: 0.7 },
  cardRow: { paddingHorizontal: 5, paddingVertical: 4 },
});

import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { ehBusca, resumoPensamento, summarizeToolInput, type ChatEvent } from '@hangar/core';
import { Icon } from '../ui/Icon';
import * as m from '../paraglide/messages';

// Um turno de raciocínio recolhido numa linha só, que abre no lugar. O ToolSearch entra no bloco
// (senão ele quebraria no meio) mas não vira linha: "select:WebSearch,WebFetch" é encanamento.
export function ThinkingBlock({ eventos }: { eventos: ChatEvent[] }) {
  const { theme } = useUnistyles();
  const [aberto, setAberto] = useState(false);
  const pensamentos = eventos.filter((e) => e.kind === 'thinking');
  const chamadas = eventos.filter((e) => e.kind === 'tool_use' && e.tool_name !== 'ToolSearch');
  const soBusca = chamadas.every((e) => ehBusca(e.tool_name));
  const n = chamadas.length;
  const rotulo =
    n === 0
      ? m.pensamento_rotulo()
      : soBusca
        ? n === 1
          ? m.pensamento_uma_busca()
          : m.pensamento_buscas({ n })
        : n === 1
          ? m.pensamento_uma_chamada()
          : m.pensamento_chamadas({ n });
  const resumo = resumoPensamento(pensamentos[0]?.text ?? '');
  return (
    <View style={styles.wrap}>
      <Pressable
        onPress={() => setAberto((v) => !v)}
        style={styles.head}
        accessibilityRole="button"
        accessibilityLabel={m.pensamento_abrir()}
        accessibilityState={{ expanded: aberto }}
      >
        <Icon name="Brain" size={13} color={theme.tokens.text.muted} />
        <Text style={[styles.rotulo, { color: theme.tokens.text.muted }]}>{rotulo}</Text>
        {!aberto ? (
          <Text style={[styles.resumo, { color: theme.tokens.text.muted }]} numberOfLines={1}>
            {resumo}
          </Text>
        ) : null}
      </Pressable>
      {aberto ? (
        <View style={[styles.corpo, { borderLeftColor: theme.tokens.border.subtle }]}>
          {eventos.map((e) =>
            e.kind === 'thinking' ? (
              <Text key={e.id} style={[styles.texto, { color: theme.tokens.text.secondary }]}>
                {e.text}
              </Text>
            ) : e.tool_name === 'ToolSearch' ? null : (
              <Text key={e.id} style={[styles.busca, { color: theme.tokens.text.muted }]}>
                · {e.tool_name}: {summarizeToolInput(e.tool_name ?? '', e.tool_input ?? {})}
              </Text>
            ),
          )}
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { paddingVertical: 2 },
  head: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingVertical: 6, paddingHorizontal: 4 },
  rotulo: { fontSize: theme.base.text.xs, fontStyle: 'italic' },
  resumo: { flex: 1, fontSize: theme.base.text.xs, fontStyle: 'italic' },
  // Fio à esquerda em vez de caixa: o pensamento é aparte da conversa, não um cartão — superfície
  // própria aqui viraria retângulo chapado por cima do papel de parede.
  corpo: { gap: 6, marginLeft: 9, paddingLeft: 14, paddingRight: 8, paddingBottom: 6, borderLeftWidth: 1 },
  texto: { fontSize: theme.base.text.sm, lineHeight: 20 },
  busca: { fontSize: theme.base.text.xs, fontFamily: theme.base.fontMono },
}));

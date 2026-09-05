import { forwardRef, useImperativeHandle, useRef, useState } from 'react';
import { ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { extractEdits, extractFilePath, summarizeToolInput, toolPhase, type ChatEvent } from '@hangar/core';
import { Sheet, type SheetRef } from '../../ui/Sheet';
import { Icon } from '../../ui/Icon';
import { EditDiff } from './EditDiff';
import { toolIcon } from './toolIcon';
import * as m from '../../paraglide/messages';

export interface ToolDetailHandle { abrir: (use: ChatEvent, result: ChatEvent | null) => void }

// Detalhe de UMA chamada: comando/diff em cima, saída embaixo. Erro de carga não existe aqui — o
// resultado já veio no evento. O que pode faltar é o `result` (chamada viva), e aí a sheet diz
// "1 rodando" em vez de ficar vazia.
export const ToolDetailSheet = forwardRef<ToolDetailHandle>(function ToolDetailSheet(_p, ref) {
  const { theme } = useUnistyles();
  const sheet = useRef<SheetRef>(null);
  const [par, setPar] = useState<{ use: ChatEvent; result: ChatEvent | null } | null>(null);
  useImperativeHandle(ref, () => ({ abrir: (use, result) => { setPar({ use, result }); sheet.current?.present(); } }));
  const use = par?.use;
  const result = par?.result ?? null;
  const edits = use ? extractEdits(use.tool_name, use.tool_input) : null;
  const fase = toolPhase(result);
  return (
    <Sheet ref={sheet} sizes={['medium', 'large']} scrollable>
      {use ? (
        <View style={styles.body}>
          <View style={styles.head}>
            <Icon name={toolIcon(use.tool_name)} size={18} />
            <Text style={[styles.titulo, { color: theme.tokens.text.primary }]} numberOfLines={2}>{use.tool_name} · {summarizeToolInput(use.tool_name, use.tool_input)}</Text>
          </View>
          <ScrollView style={styles.scroll}>
            {edits ? edits.map((e, i) => (
              <View key={i}>
                <Text style={[styles.arquivo, { color: theme.tokens.text.secondary }]}>{extractFilePath(use.tool_input)}</Text>
                <EditDiff oldText={e.oldText} newText={e.newText} />
              </View>
            )) : null}
            {!edits && typeof use.tool_input?.['command'] === 'string' ? <Text style={[styles.mono, { color: theme.tokens.accent.base }]} selectable>$ {String(use.tool_input['command'])}</Text> : null}
            {fase === 'pending' ? <Text style={[styles.mono, { color: theme.tokens.text.muted }]}>{m.formato_rodando({ n: 1 })}</Text> : null}
            {result?.result ? <Text style={[styles.mono, { color: fase === 'error' ? theme.tokens.status.error : theme.tokens.text.primary }]} selectable>{result.result}</Text> : null}
            {fase === 'done' && !result?.result && !edits ? <Text style={[styles.mono, { color: theme.tokens.text.muted }]}>{m.formato_concluido_1({ n: 1 })}</Text> : null}
          </ScrollView>
        </View>
      ) : null}
    </Sheet>
  );
});

const styles = StyleSheet.create((theme) => ({
  // A altura vem do detent da sheet; sem o flex a ScrollView cresce com o conteúdo e a saída de um
  // comando longo escorre pra fora da sheet em vez de rolar dentro dela.
  body: { flex: 1, padding: theme.base.space[4], gap: theme.base.space[3] },
  head: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  titulo: { flex: 1, fontSize: theme.base.text.sm, fontWeight: '600' },
  scroll: { flex: 1 },
  arquivo: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxs, marginBottom: 4 },
  mono: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, lineHeight: 18 },
}));

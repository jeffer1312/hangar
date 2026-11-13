import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { extractEdits, extractEditPaths, getBashOutput, summarizeToolInput, summarizeToolResult, toolPhase, type ChatEvent } from '@hangar/core';
import { Sheet, type SheetRef } from '../../ui/Sheet';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { useServers } from '../../stores/servers';
import { EditDiff } from './EditDiff';
import { toolIcon } from './toolIcon';
import { ExitCodeLine } from './ExitCodeLine';
import * as m from '../../paraglide/messages';

export interface ToolDetailHandle { abrir: (use: ChatEvent) => void }

// Detalhe de UMA chamada: comando/diff em cima, saída embaixo.
//
// Guarda só o `use`: o resultado é lido de `resultOf` a cada render, senão uma chamada aberta
// enquanto rodava ficava presa em "1 rodando" mesmo depois de o resultado chegar pelo SSE.
//
// A ScrollView é a DONA da rolagem, e o `scrollable` da sheet a adota: no Android o true-sheet
// PROCURA uma ScrollView descendente pra fixar (TrueSheetContentView.findScrollView) — sem ela o
// flag não faz nada e o conteúdo longo não rola. Não é rolagem aninhada: não há outra no caminho.
// `sessionName` liga a saída ao vivo do comando que ainda roda (sem ele, só o comando).
export const ToolDetailSheet = forwardRef<ToolDetailHandle, { resultOf: (use: ChatEvent) => ChatEvent | null; sessionName?: string; serverId?: string }>(
  function ToolDetailSheet({ resultOf, sessionName, serverId }, ref) {
    const { theme } = useUnistyles();
    const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
    const sheet = useRef<SheetRef>(null);
    const [use, setUse] = useState<ChatEvent | null>(null);
    const [aberto, setAberto] = useState(false);
    const [saidaViva, setSaidaViva] = useState<string | null>(null);
    const [erroVivo, setErroVivo] = useState<string | null>(null);
    useImperativeHandle(ref, () => ({
      abrir: (ev) => {
        setUse(ev);
        setSaidaViva(null);
        setErroVivo(null);
        setAberto(true);
        // present() rejeita quando a view nunca montou; sem o catch o toque no card não faz nada e
        // não diz por quê.
        sheet.current?.present().catch((e: unknown) => {
          console.error('ToolDetailSheet: present() falhou', e);
          setAberto(false);
          toast.erro(m.erro_desconhecido());
        });
      },
    }), []);
    const result = use ? resultOf(use) : null;
    const edits = use ? extractEdits(use.tool_name, use.tool_input) : null;
    const paths = use ? extractEditPaths(use.tool_name, use.tool_input) : null;
    const fase = toolPhase(result);
    const corFase = fase === 'error' ? theme.tokens.status.error : fase === 'pending' ? theme.tokens.accent.base : theme.tokens.text.muted;
    // Resultado vazio ainda precisa dizer o desfecho: um erro sem texto é indistinguível de sucesso,
    // e num Edit o diff sozinho parece que a edição entrou.
    const desfecho = fase === 'pending' ? m.formato_rodando({ n: 1 }) : summarizeToolResult(result, use?.tool_name);
    const comando = use && typeof use.tool_input?.['command'] === 'string' ? String(use.tool_input['command']) : null;
    // Saída parcial do Bash que ainda roda, lida a cada 2 s só com a folha aberta. A próxima leitura
    // espera a anterior voltar: backend lento não empilha pedidos.
    const pendente = fase === 'pending';
    useEffect(() => {
      if (!aberto || !pendente || !comando || !sessionName) return;
      setSaidaViva(null);
      setErroVivo(null);
      if (!server) { setErroVivo(m.chat_servidor_removido()); return; }
      let vivo = true;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const ler = async () => {
        try {
          const t = await getBashOutput(sessionName, comando, server);
          if (vivo) setSaidaViva(t);
        } catch (e) {
          // 401/404 não se curam sozinhos: repetir a cada 2 s só enchia o log. Para e diz na folha.
          console.warn('ToolDetailSheet: saída ao vivo falhou', e);
          if (vivo) setErroVivo(e instanceof Error ? e.message : String(e));
          return;
        }
        if (vivo) timer = setTimeout(ler, 2000);
      };
      void ler();
      return () => { vivo = false; clearTimeout(timer); };
    }, [aberto, pendente, comando, sessionName, serverId, server]);
    return (
      <Sheet ref={sheet} sizes={['medium', 'large']} scrollable onDismiss={() => setAberto(false)}>
        {use ? (
          <View style={styles.body}>
            <View style={styles.head}>
              <Icon name={toolIcon(use.tool_name)} size={18} color={corFase} />
              <Text style={[styles.titulo, { color: theme.tokens.text.primary }]} numberOfLines={2}>{use.tool_name} · {summarizeToolInput(use.tool_name, use.tool_input)}</Text>
            </View>
            <ScrollView style={styles.scroll}>
              {edits ? edits.map((e, i) => (
                <View key={i}>
                  <Text style={[styles.arquivo, { color: theme.tokens.text.secondary }]}>{paths?.[i]}</Text>
                  <EditDiff oldText={e.oldText} newText={e.newText} />
                </View>
              )) : null}
              {!edits && comando !== null ? <Text style={[styles.mono, { color: theme.tokens.accent.base }]} selectable>$ {comando}</Text> : null}
              {pendente && saidaViva ? (
                <>
                  <Text style={[styles.arquivo, { color: theme.tokens.text.muted }]}>{m.tool_saida()} · {m.tool_saida_ao_vivo()}</Text>
                  <Text style={[styles.mono, { color: theme.tokens.text.primary }]} selectable>{saidaViva}</Text>
                </>
              ) : null}
              {pendente && erroVivo ? (
                <Text style={[styles.arquivo, { color: theme.tokens.text.muted }]}>{m.tool_saida_ao_vivo()} · {m.quiet_erro_carregar()}: {erroVivo}</Text>
              ) : null}
              <ExitCodeLine output={result?.result} />
              {result?.result
                ? <Text style={[styles.mono, { color: fase === 'error' ? theme.tokens.status.error : theme.tokens.text.primary }]} selectable>{result.result}</Text>
                : <Text style={[styles.mono, { color: fase === 'error' ? theme.tokens.status.error : theme.tokens.text.muted }]}>{desfecho}</Text>}
            </ScrollView>
          </View>
        ) : null}
      </Sheet>
    );
  },
);

const styles = StyleSheet.create((theme) => ({
  // A altura vem do detent da sheet; sem o flex a ScrollView cresce com o conteúdo e a saída de um
  // comando longo escorre pra fora da sheet em vez de rolar dentro dela.
  body: { flex: 1, padding: theme.base.space[4], gap: theme.base.space[3] },
  head: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  titulo: { flex: 1, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  scroll: { flex: 1 },
  arquivo: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxs, marginBottom: theme.base.space[1] },
  mono: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, lineHeight: 18 },
}));

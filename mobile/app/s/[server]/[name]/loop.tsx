import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, KeyboardAvoidingView, Text, View } from 'react-native';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  createLoopForServer,
  getLoopForServer,
  refineLoopForServer,
  resolveLoopForServer,
  stopLoopForServer,
} from '@hangar/core';
import type { LoopState, Server } from '@hangar/core';
import { useServers } from '../../../../src/stores/servers';
import { LoopForm } from '../../../../src/features/loop/LoopForm';
import { LoopStatus } from '../../../../src/features/loop/LoopStatus';
import { cleanErr, isForm, isPolling } from '../../../../src/features/loop/loopSheet';
import * as m from '../../../../src/paraglide/messages';

export default function LoopSheet() {
  const { theme } = useUnistyles();
  const router = useRouter();
  const params = useLocalSearchParams<{ server: string; name: string }>();
  const serverId = Array.isArray(params.server) ? params.server[0] : (params.server ?? '');
  const sessionName = Array.isArray(params.name) ? params.name[0] : (params.name ?? '');
  const routeServer = useServers((state) => state.servers.find((server) => server.id === serverId) ?? null);
  const generation = useRef(0);

  const [loop, setLoop] = useState<LoopState | null>(null);
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [loadErr, setLoadErr] = useState('');
  const [forceForm, setForceForm] = useState(false);

  const [goal, setGoal] = useState('');
  const [checkCmd, setCheckCmd] = useState('');
  const [maxIters, setMaxIters] = useState(10);
  const [requireBranch, setRequireBranch] = useState(true);
  const [creating, setCreating] = useState(false);
  const [createErr, setCreateErr] = useState('');
  const [guideOpen, setGuideOpen] = useState(false);
  const [guideOpenSections, setGuideOpenSections] = useState<Set<number>>(() => new Set());

  const [refining, setRefining] = useState(false);
  const [refineErr, setRefineErr] = useState('');
  const [prevGoal, setPrevGoal] = useState<string | null>(null);

  const [stopBusy, setStopBusy] = useState(false);
  const [resolveBusy, setResolveBusy] = useState(false);
  const [stopError, setStopError] = useState('');

  const serverForRoute = useCallback((): Server | null => {
    return useServers.getState().servers.find((server) => server.id === serverId) ?? null;
  }, [serverId]);

  const load = useCallback(async () => {
    const server = serverForRoute();
    if (!server) {
      setLoadErr(m.compare_servidor_nao_encontrado());
      return;
    }
    const currentGeneration = generation.current;
    try {
      const response = await getLoopForServer(server, sessionName);
      if (currentGeneration !== generation.current) return;
      setLoop(response.loop);
      setSuggestions(response.suggestions);
      setLoadErr('');
    } catch (error) {
      if (currentGeneration !== generation.current) return;
      setLoadErr(cleanErr(error));
    }
  }, [serverForRoute, sessionName]);

  useEffect(() => {
    generation.current += 1;
    setLoop(null);
    setSuggestions([]);
    setLoadErr('');
    setForceForm(false);
    resetForm();
    void load();
  }, [load]);

  useEffect(() => {
    if (!isPolling(loop)) return;
    const id = setInterval(() => void load(), 3000);
    return () => clearInterval(id);
  }, [load, loop?.status]);

  function resetForm() {
    setGoal('');
    setCheckCmd('');
    setMaxIters(10);
    setRequireBranch(true);
    setCreateErr('');
    setGuideOpen(false);
    setGuideOpenSections(new Set());
    setRefining(false);
    setRefineErr('');
    setPrevGoal(null);
  }

  function toggleGuideSection(index: number) {
    setGuideOpenSections((current) => {
      const next = new Set(current);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  }

  async function refineGoal() {
    const server = serverForRoute();
    const trimmedGoal = goal.trim();
    if (!server || !trimmedGoal || refining) return;
    setRefining(true);
    setRefineErr('');
    try {
      const response = await refineLoopForServer(server, sessionName, trimmedGoal, checkCmd.trim() || null);
      setPrevGoal(goal);
      setGoal(response.goal);
    } catch (error) {
      setRefineErr(cleanErr(error));
    } finally {
      setRefining(false);
    }
  }

  async function startLoop() {
    const server = serverForRoute();
    const trimmedGoal = goal.trim();
    if (!server || !trimmedGoal || creating) return;
    setCreating(true);
    setCreateErr('');
    generation.current += 1;
    try {
      const response = await createLoopForServer(server, sessionName, {
        goal: trimmedGoal,
        check_cmd: checkCmd.trim() || null,
        max_iters: maxIters,
        require_branch: requireBranch,
      });
      setLoop(response.loop);
      setForceForm(false);
    } catch (error) {
      setCreateErr(cleanErr(error));
    } finally {
      setCreating(false);
    }
  }

  async function stopLoop() {
    const server = serverForRoute();
    if (!server || stopBusy) return;
    setStopBusy(true);
    setStopError('');
    generation.current += 1;
    try {
      const response = await stopLoopForServer(server, sessionName);
      setLoop(response.loop);
    } catch (error) {
      setStopError(cleanErr(error));
    } finally {
      setStopBusy(false);
    }
  }

  async function resolveClaim(accept: boolean) {
    const server = serverForRoute();
    if (!server || resolveBusy) return;
    setResolveBusy(true);
    setStopError('');
    generation.current += 1;
    try {
      const response = await resolveLoopForServer(server, sessionName, accept);
      setLoop(response.loop);
    } catch (error) {
      setStopError(cleanErr(error));
    } finally {
      setResolveBusy(false);
    }
  }

  function confirmStop() {
    Alert.alert(m.loop_parar_pergunta(), undefined, [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.loop_parar(), style: 'destructive', onPress: () => void stopLoop() },
    ]);
  }

  const routeMissing = !routeServer;

  return (
    <>
      <Stack.Screen options={{ headerShown: false, headerTransparent: true, sheetAllowedDetents: [0.76], contentStyle: { backgroundColor: 'transparent' } }} />
      <KeyboardAvoidingView behavior="padding" style={[styles.root, { backgroundColor: theme.tokens.bg.base }]}>
        {routeMissing ? (
          <View style={styles.missing}>
            <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert">
              {m.compare_servidor_nao_encontrado()}
            </Text>
            <Text style={[styles.back, { color: theme.tokens.accent.base }]} onPress={() => router.back()} accessibilityRole="button">
              {m.comum_voltar()}
            </Text>
          </View>
        ) : loadErr ? (
          <View style={styles.loadError}>
            <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert" selectable>
              {loadErr}
            </Text>
            <Text style={[styles.retry, { color: theme.tokens.accent.base }]} onPress={() => { setLoadErr(''); void load(); }} accessibilityRole="button">
              {m.loop_tentar_de_novo()}
            </Text>
          </View>
        ) : isForm(loop, forceForm) ? (
          <LoopForm
            goal={goal}
            checkCmd={checkCmd}
            maxIters={maxIters}
            requireBranch={requireBranch}
            suggestions={suggestions}
            creating={creating}
            createErr={createErr}
            refining={refining}
            refineErr={refineErr}
            prevGoal={prevGoal}
            guideOpen={guideOpen}
            guideOpenSections={guideOpenSections}
            onGoalChange={setGoal}
            onCheckCmdChange={setCheckCmd}
            onMaxItersChange={setMaxIters}
            onRequireBranchChange={setRequireBranch}
            onRefine={() => void refineGoal()}
            onUndoRefine={() => {
              if (prevGoal !== null) {
                setGoal(prevGoal);
                setPrevGoal(null);
              }
            }}
            onStart={() => void startLoop()}
            onToggleGuide={() => setGuideOpen((open) => !open)}
            onToggleGuideSection={toggleGuideSection}
          />
        ) : loop ? (
          <LoopStatus
            loop={loop}
            stopBusy={stopBusy}
            resolveBusy={resolveBusy}
            stopError={stopError}
            onStop={confirmStop}
            onResolve={(accept) => void resolveClaim(accept)}
            onNew={() => {
              setForceForm(true);
              resetForm();
            }}
          />
        ) : null}
      </KeyboardAvoidingView>
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
  },
  missing: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    gap: theme.base.space[2],
    padding: theme.base.space[4],
  },
  loadError: {
    padding: theme.base.space[4],
    gap: theme.base.space[2],
  },
  error: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
  back: {
    minHeight: 44,
    lineHeight: 44,
    fontSize: theme.base.text.sm,
  },
  retry: {
    minHeight: 44,
    lineHeight: 44,
    alignSelf: 'center',
    fontSize: theme.base.text.sm,
  },
}));

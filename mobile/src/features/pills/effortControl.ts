import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { parseStatusLine, getPiModels, getKimiModels, getCodexModels, setModelEffort, setPiModel, setKimiModel, setCodexModel } from '@hangar/core';
import { chatStore } from '../../stores/chat';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';
import type { PillMenuItem } from './PillMenu';
import { claudeEfforts, pillLabels, semEsforco, type Chosen } from './pills';

interface Args {
  serverId: string;
  name: string;
  provider: string | null;
  model: string | null;
  chosen: Chosen;
  onChosen: (next: Chosen) => void;
  // Fecha a folha quando a troca abre uma confirmação na conversa, que ela taparia.
  close: () => void;
}

export function useEffortControl({ serverId, name, provider, model, chosen, onChosen, close }: Args) {
  const chat = chatStore(serverId, name);
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  const statusLine = chat.use((s) => s.statusLine);
  const statusFields = useMemo(() => parseStatusLine(statusLine), [statusLine]);

  const isCodex = provider === 'codex';
  const isPi = provider === 'pi' || provider === 'omp';
  const isKimi = provider === 'kimi';
  const isClaude = !isCodex && !isPi && !isKimi;

  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [items, setItems] = useState<PillMenuItem[]>([]);
  // Modelo do Codex a que os níveis pertencem: o POST exige o modelo junto.
  const codexModel = useRef<string | null>(null);
  const readSequence = useRef(0);
  useEffect(() => () => { readSequence.current++; }, [name, server, provider]);

  const labels = pillLabels(statusFields, chosen);
  const currentModel = labels.model ?? model;
  // Haiku não usa esforço (o picker responde "Effort not supported"): seção ausente, não inútil.
  const hidden = isClaude && semEsforco(currentModel);

  const load = useCallback(async () => {
    const read = ++readSequence.current;
    codexModel.current = null;
    if (hidden) { setItems([]); setLoading(false); return; }
    if (!server) { setItems([]); setError(m.chat_servidor_removido()); setLoading(false); return; }
    setLoading(true);
    setError(null);
    setNotice(null);
    const current = labels.effort;
    const mark = (lv: string) => ({ label: lv, selected: lv === current });
    try {
      if (isPi) {
        const res = await getPiModels(name, server);
        if (read !== readSequence.current) return;
        setItems((res.levels ?? []).map(mark));
      } else if (isKimi) {
        const res = await getKimiModels(name, server);
        if (read !== readSequence.current) return;
        const curName = (currentModel ?? '').toLowerCase();
        const entry = res.models.find((mo) => mo.name.toLowerCase() === curName);
        setItems((entry?.efforts ?? []).map(mark));
      } else if (isCodex) {
        // Os níveis são do modelo ATUAL; sem modelo conhecido não há o que oferecer (cair no primeiro
        // do catálogo trocaria o modelo calado).
        const res = await getCodexModels(name, server);
        if (read !== readSequence.current) return;
        codexModel.current = res.current.model ?? null;
        const line = res.models.find((mo) => mo.model === codexModel.current);
        const active = chosen.effort ?? res.current.effort ?? line?.defaultEffort ?? null;
        setItems((line?.efforts ?? []).map((e) => ({ label: e.value, hint: e.description ?? undefined, selected: e.value === active })));
      } else {
        setItems(claudeEfforts(currentModel).map(mark));
      }
    } catch (e) {
      if (read === readSequence.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (read === readSequence.current) setLoading(false);
    }
  }, [hidden, name, server, isPi, isKimi, isCodex, labels.effort, currentModel, chosen.effort]);

  const select = useCallback(
    async (level: string) => {
      if (applying) return;
      if (!server) { setNotice(m.chat_servidor_removido()); return; }
      setApplying(true);
      setNotice(null);
      const mark = (lv: string) => setItems((cur) => cur.map((it) => ({ ...it, selected: it.label === lv })));
      try {
        if (isPi) {
          const res = await setPiModel(name, { effort: level }, server);
          // Pi ajusta ao que o modelo suporta: pinta o que voltou.
          onChosen({ ...chosen, effort: res.thinking ?? level });
          mark(res.thinking ?? level);
        } else if (isKimi) {
          const res = await setKimiModel(name, { effort: level }, server);
          onChosen({ ...chosen, effort: res.effort ?? level });
          mark(res.effort ?? level);
        } else if (isCodex) {
          if (!codexModel.current) return;
          await setCodexModel(name, codexModel.current, level, server);
          onChosen({ ...chosen, effort: level });
          mark(level);
        } else {
          const res = await setModelEffort(name, { effort: level, scope: 'session' }, server);
          // Confirmação aberta no terminal: o nível só vale se a pessoa aceitar lá.
          if (res?.pending_confirm) {
            close();
            return;
          }
          onChosen({ ...chosen, effort: level });
          mark(level);
        }
      } catch (e) {
        const status = (e as { status?: number }).status;
        const msg = e instanceof Error ? e.message : String(e);
        setNotice(status === 409 ? msg || m.composer_sessao_trabalhando() : msg);
      } finally {
        setApplying(false);
      }
    },
    [applying, name, server, isPi, isKimi, isCodex, chosen, onChosen, close],
  );

  return { hidden, value: labels.effort, items, loading, applying, error, notice, load, select };
}

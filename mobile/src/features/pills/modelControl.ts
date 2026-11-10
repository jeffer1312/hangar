import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { parseStatusLine, getModelOptions, getPiModels, getKimiModels, getCodexModels, setModelEffort, setEngineModel, setPiModel, setKimiModel, setCodexModel } from '@hangar/core';
import type { CodexModelsResponse } from '@hangar/core';
import { chatStore } from '../../stores/chat';
import { useSessions } from '../../stores/sessions';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';
import type { PillMenuItem } from './PillMenu';
import { pillLabels, type Chosen } from './pills';
import { spacedModel } from '../../chat/usage';

interface Args {
  serverId: string;
  name: string;
  provider: string | null;
  chosen: Chosen;
  onChosen: (next: Chosen) => void;
  // Fecha a folha de ajustes depois de trocar o modelo.
  close: () => void;
}

// A statusline escreve "Opus5.5·1M" e a lista do Claude, "Opus 5.5": compara sem espaço, pontuação
// e o sufixo de contexto, senão o modelo atual nunca aparece marcado.
const semEnfeite = (s: string) => s.split('·')[0].toLowerCase().replace(/[^a-z0-9]/g, '');
function mesmoModelo(nome: string, statusModel: string | null | undefined): boolean {
  return !!statusModel && semEnfeite(nome) === semEnfeite(statusModel);
}

type Catalog =
  | { kind: 'codex'; data: CodexModelsResponse }
  | { kind: 'claude' | 'engine'; names: Map<string, string> }
  | { kind: 'other' };

export function useModelControl({ serverId, name, provider, chosen, onChosen, close }: Args) {
  const chat = chatStore(serverId, name);
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  const statusLine = chat.use((s) => s.statusLine);
  // A sessão da lista traz o modelo quando a statusline não é a do Hangar ou ainda não chegou.
  const session = useSessions((s) => s.byServerRecord?.[serverId]?.find((x) => x.name === name)
    ?? s.rows.find((x) => x.serverId === serverId && x.name === name) ?? null);
  const statusFields = useMemo(() => parseStatusLine(statusLine, session), [statusLine, session]);

  const isCodex = provider === 'codex';
  // omp é o fork do Pi: mesmo seletor, mesmos endpoints.
  const isPi = provider === 'pi' || provider === 'omp';
  const isKimi = provider === 'kimi';

  const [tempError, setTempError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [items, setItems] = useState<PillMenuItem[]>([]);
  const catalog = useRef<Catalog>({ kind: 'other' });

  // Um timer só: o de uma falha anterior apagaria a mensagem da nova antes dos 8 s.
  const flashTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(flashTimer.current), []);
  const flash = useCallback((msg: string) => {
    setTempError(msg);
    clearTimeout(flashTimer.current);
    flashTimer.current = setTimeout(() => setTempError(null), 8000);
  }, []);

  const model = pillLabels(statusFields, chosen).model;
  const display = tempError ?? (model ? spacedModel(model) : m.composer_modelo());

  const load = useCallback(async (refresh = false) => {
    if (!server) { setItems([]); setError(m.chat_servidor_removido()); return; }
    setLoading(true);
    setError(null);
    setNotice(null);
    const current = chosen.model ?? statusFields?.model;
    try {
      if (isCodex) {
        const res = await getCodexModels(name, server);
        catalog.current = { kind: 'codex', data: res };
        setItems(res.models.map((mo) => ({
          id: mo.model, label: mo.displayName ?? mo.model, hint: mo.description ?? undefined,
          selected: mo.model === (chosen.model ?? res.current.model),
        })));
      } else if (isPi) {
        const res = await getPiModels(name, server);
        setItems(res.models.map((mo) => ({
          label: mo.name ?? mo.id, hint: `${mo.provider}/${mo.id}`, selected: (mo.name ?? mo.id) === current,
        })));
      } else if (isKimi) {
        const res = await getKimiModels(name, server);
        setItems(res.models.map((mo) => ({ id: mo.alias, label: mo.name, hint: mo.alias, selected: mo.name === current })));
      } else {
        const res = refresh ? await getModelOptions(name, server, { refresh: true }) : await getModelOptions(name, server);
        const isSelected = (mo: (typeof res.models)[number]) =>
          chosen.model ? (mo.name ?? mo.id) === chosen.model || mo.id === chosen.model : mesmoModelo(mo.name ?? mo.id, current);
        const models = res.models;
        catalog.current = { kind: res.kind, names: new Map(models.map((mo) => [mo.id, mo.name ?? mo.id])) };
        setItems(models.map((mo) => ({
          id: mo.id, label: mo.name ?? mo.id, hint: mo.desc ?? undefined, selected: isSelected(mo),
        })));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [name, server, isCodex, isPi, isKimi, chosen.model, statusFields?.model]);

  const select = useCallback(
    async (it: PillMenuItem) => {
      if (!server) { setNotice(m.chat_servidor_removido()); return; }
      const id = it.id ?? it.label;
      let closedEarly = false;
      setNotice(null);
      try {
        const cat = catalog.current;
        if (isCodex && cat.kind === 'codex') {
          // Trocar de modelo leva o nível padrão do NOVO: o antigo pode nem existir na lista dele.
          const md = cat.data.models.find((x) => x.model === id);
          const effort = id === cat.data.current.model
            ? (cat.data.current.effort ?? md?.defaultEffort ?? null)
            : (md?.defaultEffort ?? md?.efforts[0]?.value ?? null);
          await setCodexModel(name, id, effort, server);
          onChosen({ model: id, effort });
          close();
        } else if (isPi) {
          const alias = it.hint ?? it.label;
          const slash = alias.indexOf('/');
          const res = await setPiModel(name, { provider: slash >= 0 ? alias.slice(0, slash) : undefined, model: slash >= 0 ? alias.slice(slash + 1) : alias }, server);
          onChosen({ ...chosen, model: res.current?.name ?? res.current?.id ?? it.label, effort: res.thinking ?? chosen.effort });
          close();
        } else if (isKimi) {
          const res = await setKimiModel(name, { model: id }, server);
          onChosen({ ...chosen, model: res.current?.name ?? it.label });
          close();
        } else if (cat.kind === 'engine') {
          // Sessão de motor: o id do provedor é o rótulo, e a troca vale só nesta sessão.
          const res = await setEngineModel(name, { model: id, effort: chosen.effort ?? statusFields?.effort ?? undefined }, server);
          onChosen({ ...chosen, model: res.model });
          if (res.effort_error) {
            setNotice(m.modelo_trocado_esforco_nao({ erro: res.effort_error }));
            return;
          }
          close();
        } else {
          // Fecha antes da resposta: a troca pode abrir uma confirmação na conversa, que a folha taparia.
          closedEarly = true;
          close();
          const res = await setModelEffort(name, { model: id, scope: 'session' }, server);
          if (res?.pending_confirm) return;
          const label = cat.kind === 'claude' ? cat.names.get(id) : undefined;
          onChosen({
            ...chosen,
            model: id === 'default' ? null : label?.replace(/\s*\(1M context\)/i, '·1M').trim() || id.charAt(0).toUpperCase() + id.slice(1),
          });
        }
      } catch (e) {
        const status = (e as { status?: number }).status;
        const msg = e instanceof Error ? e.message : String(e);
        if (status === 409 || closedEarly) {
          close();
          flash(msg || m.composer_sessao_trabalhando());
        } else {
          setNotice(msg);
        }
      }
    },
    [name, server, isCodex, isPi, isKimi, chosen, onChosen, statusFields?.effort, flash, close],
  );

  const refresh = useCallback(() => load(true), [load]);
  return { model, display, failed: tempError !== null, items, loading, error, notice, load, select,
    refresh: !isCodex && !isPi && !isKimi ? refresh : undefined };
}

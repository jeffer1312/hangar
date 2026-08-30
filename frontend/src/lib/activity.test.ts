import { describe, it, expect } from 'vitest';
import { createActivityFolder, rotuloDeComando } from './activity';
import type { ChatEvent } from './types';

let seq = 0;
// Agent (launch bloqueante em background) -> vira uma linha no painel.
const launch = (tuid: string): ChatEvent =>
  ({ kind: 'tool_use', id: `e${seq++}`, tool_name: 'Agent', tool_use_id: tuid, tool_input: { description: 'Assessment A — design review' } });
// tool_result imediato do launch: traz o agentId no texto "Async agent launched...".
const launched = (tuid: string, agentId: string): ChatEvent =>
  ({ kind: 'tool_result', id: `e${seq++}`, tool_use_id: tuid, result: `Async agent launched successfully. agentId: ${agentId} (internal ID)` });
// fim real: <task-notification> numa user_msg com o <task-id> = agentId.
const done = (agentId: string): ChatEvent =>
  ({ kind: 'user_msg', id: `e${seq++}`, text: `<task-notification>\n<task-id>${agentId}</task-id>\n<status>completed</status>\n</task-notification>` });

function run(events: ChatEvent[]) {
  const f = createActivityFolder();
  f.reset(events);
  return f.snapshot();
}

describe('activity — pareamento de agente background', () => {
  it('marca terminado quando o fim vem DEPOIS do launch (ordem normal)', () => {
    const s = run([launch('tu1'), launched('tu1', 'aa0777'), done('aa0777')]);
    expect(s.runningAgents).toBe(0);
    expect(s.agents[0].running).toBe(false);
  });

  it('marca terminado quando o fim vem ANTES do launch (regressao: troca de transcript no /clear)', () => {
    // o <task-notification> do fim e processado no fold ANTES do tool_result do launch. Antes do fix
    // (pareamento dependente de ordem) o par nunca fechava e o agente ficava "rodando" pra sempre.
    const s = run([launch('tu1'), done('aa0777'), launched('tu1', 'aa0777')]);
    expect(s.runningAgents).toBe(0);
  });

  it('segue rodando enquanto nao chega o evento de fim', () => {
    const s = run([launch('tu1'), launched('tu1', 'aa0777')]);
    expect(s.runningAgents).toBe(1);
    expect(s.agents[0].running).toBe(true);
  });
});

describe('activity — AgentSwarm (Kimi)', () => {
  // Lote: N subagentes de uma vez, um por item. Antes disto o AgentSwarm nao caia em nenhum case e
  // o painel de Atividade dizia "nada rolando agora" com quatro agentes trabalhando.
  const swarm = (tuid: string, itens: string[]): ChatEvent =>
    ({ kind: 'tool_use', id: `e${seq++}`, tool_name: 'AgentSwarm', tool_use_id: tuid,
       tool_input: { description: 'Assistir os 4 vídeos da PM', subagent_type: 'explore', items: itens } });

  it('vira uma linha, com a contagem de itens do lote', () => {
    const s = run([swarm('tu9', ['a.mp4', 'b.mp4', 'c.mp4', 'd.mp4'])]);
    expect(s.agents).toHaveLength(1);
    expect(s.agents[0].description).toContain('Assistir os 4 vídeos da PM');
    expect(s.agents[0].description).toContain('4');
    expect(s.agents[0].subagentType).toBe('explore');
  });

  it('sem items, nao inventa contagem', () => {
    const s = run([swarm('tu9', [])]);
    expect(s.agents[0].description).toBe('Assistir os 4 vídeos da PM');
  });
});

// ── shells de fundo (`Bash` com run_in_background) ──────────────────────────────────────────
// Mesma mecânica do agente em background: o tool_result chega NA HORA com o id, e o fim real vem
// como <task-notification>. O que muda é a armadilha: o texto do lançamento pode aparecer na SAÍDA
// de um comando qualquer, e um resultado desses não pode virar um shell que nunca fecha.
const bash = (tuid: string, cmd: string, fundo = true): ChatEvent =>
  ({ kind: 'tool_use', id: `e${seq++}`, tool_name: 'Bash', tool_use_id: tuid,
     tool_input: { command: cmd, run_in_background: fundo } });
const bashOk = (tuid: string, bgId: string): ChatEvent =>
  ({ kind: 'tool_result', id: `e${seq++}`, tool_use_id: tuid,
     result: `Command running in background with ID: ${bgId}. Output is being written to /tmp/x.output` });
const resultado = (tuid: string, texto: string): ChatEvent =>
  ({ kind: 'tool_result', id: `e${seq++}`, tool_use_id: tuid, result: texto });

describe('activity — shells de fundo', () => {
  it('conta como rodando até a notificação de fim chegar', () => {
    const f = createActivityFolder();
    f.reset([bash('tb1', 'uv run pytest -q'), bashOk('tb1', 'bxyz')]);
    expect(f.snapshot().runningShells).toBe(1);
    expect(f.snapshot().shells[0].command).toBe('uv run pytest -q');
    f.push(done('bxyz'));
    expect(f.snapshot().runningShells).toBe(0);
    expect(f.snapshot().shells[0].running).toBe(false);
  });

  it('o fim que chega ANTES do lançamento fecha o par igual', () => {
    // Mesmo reordenamento que o reseed do /clear já causava nos agentes.
    const s = run([done('bzzz'), bash('tb2', 'npm run build'), bashOk('tb2', 'bzzz')]);
    expect(s.runningShells).toBe(0);
  });

  it('Bash comum nao vira shell de fundo', () => {
    const s = run([bash('tb3', 'ls', false), resultado('tb3', 'a\nb')]);
    expect(s.shells).toHaveLength(0);
  });

  it('resultado de primeiro plano que CITA o id de um shell real nao rouba o par dele', () => {
    // É este o cenário que o `bgPendente` protege, e ele é estreito: um comando comum (um `grep`
    // neste repo devolve a frase do lançamento) cujo texto cita o MESMO id de um shell de verdade.
    // Sem o guard, esse resultado sequestraria o mapa `bgAgent` — o fim de `B1` fecharia o tool_use
    // do grep, e o shell verdadeiro ficaria "rodando" pra sempre, sem nada na tela dizendo isso.
    const s = run([
      bash('tb4', 'uv run pytest -q'), bashOk('tb4', 'B1'),
      bash('tb5', 'grep -rn "background with ID" .', false),
      resultado('tb5', 'activity.test.ts: Command running in background with ID: B1'),
      done('B1'),
    ]);
    expect(s.runningShells).toBe(0);
    expect(s.shells.find((x) => x.id === 'tb4')?.running).toBe(false);
  });

  it('pediu fundo e o harness RECUSOU: termina ali, nao fica rodando', () => {
    const s = run([bash('tb5', 'sleep 60'),
                   resultado('tb5', 'Blocked: sleep 60 followed by...')]);
    expect(s.shells).toHaveLength(1);
    expect(s.runningShells).toBe(0);
  });

  it('rodando primeiro na lista', () => {
    const s = run([bash('tb6', 'primeiro'), bashOk('tb6', 'b1'), done('b1'),
                   bash('tb7', 'segundo'), bashOk('tb7', 'b2')]);
    expect(s.shells.map((x) => x.command)).toEqual(['segundo', 'primeiro']);
  });
});

// Casos REAIS desta máquina (os que apareceram no painel), não inventados: a lista é estreita e o
// encanamento comia a linha inteira antes do comando aparecer.
describe('rotuloDeComando', () => {
  it('tira o cd da frente, a redirecao e o tail do fim', () => {
    expect(rotuloDeComando('cd /home/jefferson/Projetos/hangar/backend && uv run pytest -q 2>&1 | tail -4'))
      .toBe('uv run pytest -q');
  });

  it('tira >/dev/null e 2>&1 do meio', () => {
    expect(rotuloDeComando('npm --prefix frontend run build >/dev/null 2>&1'))
      .toBe('npm --prefix frontend run build');
  });

  it('cd com caminho entre aspas tambem sai', () => {
    expect(rotuloDeComando('cd "/home/u/Área de trabalho" && ls')).toBe('ls');
  });

  it('comando sem encanamento fica igual', () => {
    expect(rotuloDeComando('sleep 240; echo pronto-a')).toBe('sleep 240; echo pronto-a');
  });

  it('nao come o que da SENTIDO ao comando', () => {
    // A poda é só de encanamento. Um `2>` pra ARQUIVO, um `| grep` e o `-q` continuam ali: sem
    // eles a linha diria que o comando faz outra coisa.
    expect(rotuloDeComando('pytest 2> erros.log | grep FAIL'))
      .toBe('pytest 2> erros.log | grep FAIL');
  });

  it('podar tudo devolve o cru em vez de linha vazia', () => {
    expect(rotuloDeComando('>/dev/null')).toBe('>/dev/null');
  });
});

// O plano do Codex (`update_plan`). Mesma natureza do TodoWrite: a lista INTEIRA a cada chamada,
// a última vence. Campos copiados de rollouts reais desta máquina: `plan[].step` + `status`.
describe('activity — plano do Codex', () => {
  const plano = (itens: { step: string; status: string }[]): ChatEvent =>
    ({ kind: 'tool_use', id: `p${seq++}`, tool_name: 'update_plan', tool_use_id: `c${seq}`,
       tool_input: { plan: itens } });

  it('vira lista de tarefas com os mesmos status das outras listas', () => {
    const s = run([plano([
      { step: 'Ler regras do repositório', status: 'completed' },
      { step: 'Validar app-server', status: 'in_progress' },
      { step: 'Implementar', status: 'pending' },
    ])]);
    expect(s.tasks.map((t) => [t.title, t.status])).toEqual([
      ['Ler regras do repositório', 'completed'],
      ['Validar app-server', 'in_progress'],
      ['Implementar', 'pending'],
    ]);
  });

  it('a última chamada vence, como no TodoWrite', () => {
    const s = run([
      plano([{ step: 'A', status: 'in_progress' }]),
      plano([{ step: 'A', status: 'completed' }, { step: 'B', status: 'in_progress' }]),
    ]);
    expect(s.tasks.map((t) => [t.title, t.status])).toEqual([['A', 'completed'], ['B', 'in_progress']]);
  });

  it('item sem `step` é descartado em vez de virar linha sem título', () => {
    const s = run([plano([{ step: 'A', status: 'pending' }, { status: 'pending' } as never])]);
    expect(s.tasks.map((t) => t.title)).toEqual(['A']);
  });
});

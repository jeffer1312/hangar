// @vitest-environment happy-dom
// format.ts importa locale.ts (getLocale do Paraglide com estrategia localStorage) — o
// formato de data/hora e numero passa a depender do idioma escolhido, e o teste precisa do DOM.
import { describe, it, expect, beforeEach } from 'vitest';
import {
  abbrevNum, attentionFeed, countAwaiting, effectiveGroupBy, fmtWhen, groupSelectedByServer, initials, nextAwaiting,
  pedeMarcacao,
  projectKey, projectLabel, encodeCompareIds, parseCompareIds, latestAssistantEvent, resetsIn, relativeTime,
  clusterByPair, sortSessions, bubblesFromTail, ctxWindow, fileKind, fmtBytes, providerName, providerTag,
  untrackedReason,
  summarizeText, summarizeToolInput, summarizeToolResult, toolPhase, toolGroupLabel, toolGroupCounts,
  rotuloEstado,
  splitTodoBlock, parseImageMessage, parseCanal, basename,
} from './format';
import type { ChatEvent, State } from './types';
import { overwriteGetLocale } from '../paraglide/runtime';
import { intlLocale } from './locale';

// Default dos testes antigos: eles foram escritos esperando pt-BR. Quem troca de idioma seta
// o locale DENTRO do teste e o beforeEach repoe o pt a cada it — nenhum estado vaza entre tests.
beforeEach(() => overwriteGetLocale(() => 'pt'));

describe('abbrevNum', () => {
  it('abbreviates millions', () => {
    expect(abbrevNum(3_668_662)).toBe('3.7M');
  });
  it('abbreviates billions', () => {
    expect(abbrevNum(1_539_946_914)).toBe('1.5B');
  });
  it('abbreviates thousands', () => {
    expect(abbrevNum(12_500)).toBe('12.5K');
  });
  it('leaves small numbers as-is', () => {
    expect(abbrevNum(999)).toBe('999');
  });
  it('drops trailing .0', () => {
    expect(abbrevNum(2_000_000)).toBe('2M');
  });
});

describe('initials', () => {
  it('sessões irmãs não colidem: o sufixo numérico entra na sigla', () => {
    expect(initials('claude-cockpit')).toBe('CC');
    expect(initials('claude-cockpit-2')).toBe('C2');
    expect(initials('hangar-logo')).toBe('HL');
    expect(initials('api-3')).toBe('A3');
    // dois dígitos ainda cabem; três não são sufixo de sessão irmã, é nome
    expect(initials('worker-12')).toBe('W12');
    // três+ palavras: a letra sai da palavra ANTES do número, senão a família inteira colide
    expect(initials('svc-mailer-2')).toBe('M2');
    expect(initials('svc-report-ai-2')).toBe('A2');
    expect(initials('jeffer1312')).toBe('JE');
  });

  it('família com número no meio e sufixo textual: a letra do sufixo distingue (task-4021-api → TA)', () => {
    expect(initials('task-4021-api')).toBe('TA');
    expect(initials('task-4021-front')).toBe('TF');
    expect(initials('task-4021-sync-b')).toBe('TS');
    expect(initials('task-4020-kimi')).toBe('TK');
    // sem número no meio, nada muda: o fallback de sempre continua valendo
    expect(initials('claude-cockpit-extra')).toBe('CC');
  });

  it('takes first letter of each of two words', () => {
    expect(initials('api-front')).toBe('AF');
  });
  it('splits on non-alphanumeric separators', () => {
    expect(initials('app_web')).toBe('AW');
    expect(initials('foo bar baz')).toBe('FB');
  });
  it('uses first two chars for a single word', () => {
    expect(initials('jeffer1312')).toBe('JE');
  });
  it('uppercases', () => {
    expect(initials('vps')).toBe('VP');
  });
  it('returns empty string for empty input', () => {
    expect(initials('')).toBe('');
    // nome só com separadores: não pode virar sigla vazia (o chip do trilho ficaria em branco)
    expect(initials('---')).toBe('--');
    expect(initials('___')).toBe('__');
  });
});

describe('countAwaiting', () => {
  it('counts only awaiting_input sessions', () => {
    const sessions = [
      { state: 'awaiting_input' as const },
      { state: 'working' as const },
      { state: 'awaiting_input' as const },
      { state: 'idle' as const },
      { state: 'dead' as const },
    ];
    expect(countAwaiting(sessions)).toBe(2);
  });

  it('returns 0 for an empty list', () => {
    expect(countAwaiting([])).toBe(0);
  });

  it('returns 0 when none are awaiting', () => {
    expect(countAwaiting([{ state: 'working' as const }, { state: 'idle' as const }])).toBe(0);
  });
});

describe('nextAwaiting', () => {
  it('returns null when nothing is awaiting', () => {
    const sessions = [{ name: 'a', state: 'idle' as const }, { name: 'b', state: 'working' as const }];
    expect(nextAwaiting(sessions, 'a')).toBeNull();
  });

  it('jumps to the single awaiting session when current is not it', () => {
    const sessions = [
      { name: 'a', state: 'idle' as const },
      { name: 'b', state: 'awaiting_input' as const },
    ];
    expect(nextAwaiting(sessions, 'a')).toBe('b');
  });

  it('wraps around from the last awaiting session back to the first', () => {
    const sessions = [
      { name: 'a', state: 'awaiting_input' as const },
      { name: 'b', state: 'awaiting_input' as const },
      { name: 'c', state: 'awaiting_input' as const },
    ];
    expect(nextAwaiting(sessions, 'c')).toBe('a');
  });

  it('skips past the current session when it is itself awaiting', () => {
    const sessions = [
      { name: 'a', state: 'awaiting_input' as const },
      { name: 'b', state: 'awaiting_input' as const },
      { name: 'c', state: 'awaiting_input' as const },
    ];
    expect(nextAwaiting(sessions, 'a')).toBe('b');
  });

  it('returns itself when it is the only awaiting session', () => {
    const sessions = [{ name: 'a', state: 'awaiting_input' as const }];
    expect(nextAwaiting(sessions, 'a')).toBe('a');
  });
});

describe('pedeMarcacao', () => {
  it('reconhece o menu de marcação pela caixa na frente da opção', () => {
    // Caso vivo de 25/08/2026: a tira de atenção mostrou "[ ] backend" como resposta rápida.
    expect(pedeMarcacao(['[✓] app de desktop', '[ ] backend', '[ ] Type something'])).toBe(true);
  });

  it('basta UMA opção com caixa — o escape do AskUserQuestion vem sem ela', () => {
    expect(pedeMarcacao(['[ ] app de desktop', '[ ] backend', 'Chat about this'])).toBe(true);
  });

  it('escolha única não pede marcação', () => {
    expect(pedeMarcacao(['Gerar miniatura no backend', 'Remover exibição no front'])).toBe(false);
  });

  it('lista vazia ou ausente não pede marcação', () => {
    expect(pedeMarcacao([])).toBe(false);
    expect(pedeMarcacao(undefined)).toBe(false);
    expect(pedeMarcacao(null)).toBe(false);
  });

  it('colchete no MEIO do texto não conta — só o que abre a linha', () => {
    expect(pedeMarcacao(['Rodar com [--force]', 'Cancelar'])).toBe(false);
  });
});

describe('attentionFeed', () => {
  it('keeps only awaiting_input sessions', () => {
    const sessions = [
      { name: 'a', state: 'working' as const, last_activity: 1 },
      { name: 'b', state: 'awaiting_input' as const, last_activity: 2 },
      { name: 'c', state: 'idle' as const, last_activity: 3 },
      { name: 'd', state: 'awaiting_input' as const, last_activity: 4 },
    ];
    expect(attentionFeed(sessions).map((s) => s.name)).toEqual(['b', 'd']);
  });

  it('sorts oldest-waiting (smallest last_activity) first', () => {
    const sessions = [
      { name: 'newer', state: 'awaiting_input' as const, last_activity: 200 },
      { name: 'older', state: 'awaiting_input' as const, last_activity: 100 },
      { name: 'mid', state: 'awaiting_input' as const, last_activity: 150 },
    ];
    expect(attentionFeed(sessions).map((s) => s.name)).toEqual(['older', 'mid', 'newer']);
  });

  it('merges across servers (any shape with the fields) and puts missing last_activity last', () => {
    const sessions = [
      { name: 'z', state: 'awaiting_input' as const, last_activity: null, serverId: 's2' },
      { name: 'a', state: 'awaiting_input' as const, last_activity: 50, serverId: 's1' },
    ];
    expect(attentionFeed(sessions).map((s) => s.name)).toEqual(['a', 'z']);
  });

  it('breaks ties by name for a stable order', () => {
    const sessions = [
      { name: 'beta', state: 'awaiting_input' as const, last_activity: 10 },
      { name: 'alpha', state: 'awaiting_input' as const, last_activity: 10 },
    ];
    expect(attentionFeed(sessions).map((s) => s.name)).toEqual(['alpha', 'beta']);
  });

  it('returns an empty list when nothing is awaiting', () => {
    expect(attentionFeed([{ name: 'a', state: 'idle' as const }])).toEqual([]);
  });
});

describe('projectKey', () => {
  it('strips a trailing slash', () => {
    expect(projectKey('/home/user/repo/')).toBe('/home/user/repo');
  });
  it('keeps the root path as-is', () => {
    expect(projectKey('/')).toBe('/');
  });
  it('keeps a nested path as-is (no trailing slash)', () => {
    expect(projectKey('/home/user/repo/backend')).toBe('/home/user/repo/backend');
  });
  it('same cwd with/without trailing slash -> same key', () => {
    expect(projectKey('/a/b/c')).toBe(projectKey('/a/b/c/'));
  });
  it('falls back to a fixed sentinel when there is no cwd', () => {
    const noCwd = projectKey(undefined);
    expect(projectKey(null)).toBe(noCwd);
    expect(projectKey('')).toBe(noCwd);
    expect(noCwd).not.toBe(projectKey('/'));
  });
});

describe('groupSelectedByServer', () => {
  const sessions = [
    { name: 'a', serverId: 's1' },
    { name: 'b', serverId: 's1' },
    { name: 'c', serverId: 's2' },
    { name: 'd', serverId: 's2' },
  ];

  it('groups selected names by their owning server', () => {
    const selected = new Set(['s1:a', 's2:c', 's2:d']);
    const grouped = groupSelectedByServer(sessions, selected);
    expect(grouped.get('s1')).toEqual(['a']);
    expect(grouped.get('s2')).toEqual(['c', 'd']);
    expect(grouped.size).toBe(2);
  });

  it('omits servers with nothing selected', () => {
    const grouped = groupSelectedByServer(sessions, new Set(['s1:a']));
    expect(grouped.has('s2')).toBe(false);
  });

  it('returns an empty map when nothing is selected', () => {
    expect(groupSelectedByServer(sessions, new Set()).size).toBe(0);
  });

  it('ignores selection keys that do not match any session', () => {
    const grouped = groupSelectedByServer(sessions, new Set(['s1:a', 's3:ghost']));
    expect(grouped.size).toBe(1);
    expect(grouped.get('s1')).toEqual(['a']);
  });
});

describe('effectiveGroupBy', () => {
  it('keeps the preference when there are 2+ servers', () => {
    expect(effectiveGroupBy('server', 2)).toBe('server');
    expect(effectiveGroupBy('project', 3)).toBe('project');
  });
  it('falls back to a flat list when "server" has nothing to separate', () => {
    expect(effectiveGroupBy('server', 1)).toBe('none');
    expect(effectiveGroupBy('server', 0)).toBe('none');
  });
  it('never overrides "project" or "none" — both are valid with any server count', () => {
    expect(effectiveGroupBy('project', 1)).toBe('project');
    expect(effectiveGroupBy('none', 1)).toBe('none');
    expect(effectiveGroupBy('none', 3)).toBe('none');
  });
});

describe('basename', () => {
  it('pega o último segmento de um caminho unix', () => {
    expect(basename('/home/user/repo')).toBe('repo');
    expect(basename('/home/user/repo/')).toBe('repo');
  });
  it('pega o último segmento de um caminho do WINDOWS', () => {
    // Sem isto o cwd voltava INTEIRO — a sessão criada pelo app nascia chamada
    // `C--Sistemas-DotNet-PssBackend` (o caminho todo, depois do sanitizador do nome).
    expect(basename('C:\\Sistemas\\DotNet\\PssBackend')).toBe('PssBackend');
    expect(basename('C:\\Sistemas\\DotNet\\PssBackend\\')).toBe('PssBackend');
    expect(basename('\\\\servidor\\share\\projeto')).toBe('projeto');
  });
  it('não quebra por contrabarra num caminho unix — lá ela é nome de arquivo válido', () => {
    expect(basename('/home/user/pasta\\estranha')).toBe('pasta\\estranha');
  });
});

describe('projectLabel', () => {
  it('is the basename for a trailing-slash path', () => {
    expect(projectLabel('/home/user/repo/')).toBe('repo');
  });
  it('is the root path itself when cwd is root', () => {
    expect(projectLabel('/')).toBe('/');
  });
  it('is the basename for a nested path', () => {
    expect(projectLabel('/home/user/repo/backend')).toBe('backend');
  });
  it('has a fixed label when there is no cwd', () => {
    expect(projectLabel(undefined)).toBe('sem projeto');
    expect(projectLabel(null)).toBe('sem projeto');
  });
});

describe('encodeCompareIds / parseCompareIds', () => {
  it('round-trips a normal list', () => {
    const ids = [{ serverId: 's1', name: 'work' }, { serverId: 's2', name: 'home' }];
    expect(parseCompareIds(encodeCompareIds(ids))).toEqual(ids);
  });

  it('escapes literal separators inside ids/names so they never collide with , or :', () => {
    const ids = [{ serverId: 'a:b', name: 'x,y' }, { serverId: 'c,d', name: 'e:f' }];
    const encoded = encodeCompareIds(ids);
    expect(encoded).not.toMatch(/a:b|x,y|c,d|e:f/); // valores crus não sobrevivem ao encode
    expect(parseCompareIds(encoded)).toEqual(ids);
  });

  it('parses an empty param as an empty list', () => {
    expect(parseCompareIds('')).toEqual([]);
  });

  it('drops malformed pairs (no colon, or missing side)', () => {
    expect(parseCompareIds('noColonHere')).toEqual([]);
    expect(parseCompareIds(':nome')).toEqual([]); // serverId vazio
    expect(parseCompareIds('srv:')).toEqual([]); // nome vazio
  });

  it('keeps well-formed pairs alongside malformed ones', () => {
    const encoded = `${encodeURIComponent('s1')}:${encodeURIComponent('a')},garbage,${encodeURIComponent('s2')}:${encodeURIComponent('b')}`;
    expect(parseCompareIds(encoded)).toEqual([{ serverId: 's1', name: 'a' }, { serverId: 's2', name: 'b' }]);
  });
});

describe('latestAssistantEvent', () => {
  const asst = (id: string, text: string): ChatEvent => ({ kind: 'assistant_msg', id, text });
  const userMsg = (id: string, text: string): ChatEvent => ({ kind: 'user_msg', id, text });

  it('returns the last assistant_msg with text', () => {
    const events = [asst('1', 'oi'), userMsg('2', 'e ai'), asst('3', 'tudo bem')];
    expect(latestAssistantEvent(events)?.id).toBe('3');
  });

  it('skips assistant_msg entries without text', () => {
    const events = [asst('1', 'primeira'), { kind: 'assistant_msg', id: '2' } as ChatEvent];
    expect(latestAssistantEvent(events)?.id).toBe('1');
  });

  it('returns null when there is no assistant_msg', () => {
    expect(latestAssistantEvent([userMsg('1', 'oi')])).toBeNull();
  });

  it('returns null for an empty list', () => {
    expect(latestAssistantEvent([])).toBeNull();
  });
});

describe('resetsIn', () => {
  const now = () => Date.now() / 1000;
  it('formata instante FUTURO como "em X" (o bug era cair em "agora")', () => {
    // +5s de folga: o now() daqui e o Date.now() interno do resetsIn leem relógios com ms de
    // diferença — sem folga o floor() caía pro degrau de baixo sempre que um ms virava entre as
    // duas leituras ("em 6 d" em vez de "em 7 d"; flake real observado).
    expect(resetsIn(now() + 7 * 86400 + 5)).toBe('em 7 d');
    expect(resetsIn(now() + 2 * 3600 + 5)).toBe('em 2 h');
    expect(resetsIn(now() + 30 * 60 + 5)).toBe('em 30 min');
  });
  it('arredonda pra pelo menos 1 min e trata falsy/passado como vazio', () => {
    expect(resetsIn(now() + 20)).toBe('em 1 min');   // <1min -> não vira "em 0 min"
    expect(resetsIn(null)).toBe('');
    expect(resetsIn(now() - 3600)).toBe('');          // já passou -> vazio
  });
});

describe('fmtWhen', () => {
  it('returns empty string for falsy timestamps', () => {
    expect(fmtWhen(0)).toBe('');
    expect(fmtWhen(null)).toBe('');
    expect(fmtWhen(undefined)).toBe('');
  });

  it('formats epoch SECONDS (x1000) as a local short date-time', () => {
    const ts = 1_700_000_000; // epoch em segundos
    // fmtWhen usa o locale ESCOLHIDO no app (intlLocale), nao o do navegador (array vazio).
    const expected = new Date(ts * 1000).toLocaleString(intlLocale(), { dateStyle: 'short', timeStyle: 'short' });
    expect(fmtWhen(ts)).toBe(expected);
    expect(fmtWhen(ts)).not.toBe('');
  });

  it('fmtWhen segue o idioma: em ingles sai no formato en-US, nao no do SO', () => {
    // Regressao real pega por este caso: a fonte ficou em toLocaleString([], ...) (locale do SO)
    // enquanto o teste comparava com intlLocale() — em maquina pt-BR os dois coincidiam e o teste
    // passava verde por acidente; com LC_ALL=C (ou qualquer SO en) ele fechava em vermelho.
    const ts = 1_700_000_000;
    overwriteGetLocale(() => 'en');
    expect(fmtWhen(ts)).toBe(new Date(ts * 1000).toLocaleString('en-US', { dateStyle: 'short', timeStyle: 'short' }));
    overwriteGetLocale(() => 'pt');
    expect(fmtWhen(ts)).toBe(new Date(ts * 1000).toLocaleString('pt-BR', { dateStyle: 'short', timeStyle: 'short' }));
  });
});

describe('clusterByPair', () => {
  const S = (name: string, gid: string | null = null, task: string | null = null) =>
    ({ name, pair_gid: gid, pair_task: task });

  it('sessão sem grupo vira linha solo', () => {
    const rows = clusterByPair([S('a'), S('b')]);
    expect(rows).toEqual([
      { kind: 'session', session: S('a'), gid: null },
      { kind: 'session', session: S('b'), gid: null },
    ]);
  });

  it('membros do mesmo gid viram header + linhas, na posição do 1º', () => {
    const rows = clusterByPair([S('a', 'g1', 'TICKET-0000'), S('solo'), S('b', 'g1')]);
    expect(rows[0]).toEqual({ kind: 'header', gid: 'g1', label: 'TICKET-0000', count: 2 });
    expect(rows[1]).toMatchObject({ kind: 'session', gid: 'g1' });
    expect(rows[2]).toMatchObject({ kind: 'session', gid: 'g1' });
    expect(rows[3]).toEqual({ kind: 'session', session: S('solo'), gid: null }); // solo depois
  });

  it('N grupos = N clusters distintos', () => {
    const rows = clusterByPair([S('a', 'g1'), S('c', 'g2'), S('b', 'g1'), S('d', 'g2')]);
    const headers = rows.filter((r) => r.kind === 'header');
    expect(headers.map((h: any) => h.gid)).toEqual(['g1', 'g2']);
  });

  it('label cai nos nomes quando não há task', () => {
    const rows = clusterByPair([S('front', 'g1'), S('back', 'g1')]);
    expect((rows[0] as any).label).toBe('front, back');
  });
});

// A ordenação foi EXTRAÍDA (Sidebar + SessionList) justamente porque as duas listas já divergiram na
// ordenação no passado. Extrair sem fixar a ordem num teste deixaria o mesmo bug livre pra voltar —
// é isto que estes testes trancam.
describe('sortSessions', () => {
  const S = (name: string, state: State = 'idle') => ({ name, state });

  it('awaiting_input vem primeiro, independente do nome', () => {
    const out = sortSessions([S('aaa'), S('zzz', 'awaiting_input'), S('bbb')]);
    expect(out.map((s) => s.name)).toEqual(['zzz', 'aaa', 'bbb']);
  });

  it('desempata alfabeticamente dentro do mesmo grupo', () => {
    const out = sortSessions([S('charlie'), S('alpha'), S('bravo')]);
    expect(out.map((s) => s.name)).toEqual(['alpha', 'bravo', 'charlie']);
  });

  it('alfabético também ENTRE os que aguardam (não só entre os demais)', () => {
    const out = sortSessions([S('zeta', 'awaiting_input'), S('alfa', 'awaiting_input'), S('m')]);
    expect(out.map((s) => s.name)).toEqual(['alfa', 'zeta', 'm']);
  });

  it('working/idle/dead não se ordenam entre si — só awaiting_input sobe', () => {
    const out = sortSessions([S('d', 'dead'), S('c', 'working'), S('b', 'idle'), S('a', 'awaiting_input')]);
    expect(out.map((s) => s.name)).toEqual(['a', 'b', 'c', 'd']);
  });

  it('é estável: empate total preserva a ordem de entrada', () => {
    // Mesmo nome + mesmo estado = comparador devolve 0 nos dois critérios. `id` distingue quem é quem.
    const list = [
      { name: 'dup', state: 'idle' as State, id: 1 },
      { name: 'dup', state: 'idle' as State, id: 2 },
      { name: 'dup', state: 'idle' as State, id: 3 },
    ];
    expect(sortSessions(list).map((s) => s.id)).toEqual([1, 2, 3]);
  });

  it('não muta a entrada (devolve lista nova)', () => {
    const list = [S('zzz'), S('aaa')];
    const out = sortSessions(list);
    expect(list.map((s) => s.name)).toEqual(['zzz', 'aaa']);   // original intacto
    expect(out).not.toBe(list);
  });

  it('lista vazia -> lista vazia', () => {
    expect(sortSessions([])).toEqual([]);
  });
});

describe('bubblesFromTail', () => {
  const E = (id: string, kind: ChatEvent['kind'], text?: string) => ({ id, kind, text }) as ChatEvent;

  it('descarta o assistant_msg órfão antes do 1º user_msg', () => {
    // Janela REAL medida ao vivo numa sessão que acabou de usar ferramentas: a cauda de 8 começa num
    // assistant_msg cujo prompt ficou de fora. O card não pode desenhá-lo (resposta sem pergunta);
    // a rota TEM que devolvê-lo (é o que a espiada do hover procura). Daí o corte morar aqui.
    const tail = [
      E('1', 'assistant_msg', 'PIZZA-ANTERIOR'), E('2', 'user_msg', 'rode os comandos'),
      E('3', 'tool_use'), E('4', 'tool_result'), E('5', 'tool_use'), E('6', 'tool_result'),
    ];
    expect(bubblesFromTail(tail).map((e) => e.id)).toEqual(['2']);
  });

  it('mantém tudo quando a janela já começa num user_msg', () => {
    const tail = [E('1', 'user_msg', 'oi'), E('2', 'assistant_msg', 'olá'), E('3', 'tool_use')];
    expect(bubblesFromTail(tail).map((e) => e.id)).toEqual(['1', '2']);
  });

  it('sem user_msg na janela -> devolve as bolhas que houver (card vazio é pior)', () => {
    const tail = [E('1', 'assistant_msg', 'so resposta'), E('2', 'tool_use'), E('3', 'tool_result')];
    expect(bubblesFromTail(tail).map((e) => e.id)).toEqual(['1']);
  });

  it('filtra tool_use/tool_result e bolha sem texto', () => {
    const tail = [E('1', 'user_msg', 'oi'), E('2', 'assistant_msg'), E('3', 'tool_result'),
                  E('4', 'assistant_msg', 'pronto')];
    expect(bubblesFromTail(tail).map((e) => e.id)).toEqual(['1', '4']);
  });

  it('não muta a entrada', () => {
    const tail = [E('1', 'assistant_msg', 'orfa'), E('2', 'user_msg', 'oi')];
    bubblesFromTail(tail);
    expect(tail.map((e) => e.id)).toEqual(['1', '2']);
  });

  it('cauda vazia -> vazio', () => {
    expect(bubblesFromTail([])).toEqual([]);
  });
});

describe('ctxWindow', () => {
  it('mostra M inteiro quando exato (a janela do Opus5 aparecia como "1000k")', () => {
    expect(ctxWindow(1_000_000)).toBe('1M');
  });

  it('mostra decimal quando nao e M redondo', () => {
    expect(ctxWindow(1_500_000)).toBe('1.5M');
  });

  it('mantem k abaixo de 1M', () => {
    expect(ctxWindow(200_000)).toBe('200k');
    expect(ctxWindow(258_400)).toBe('258k');
  });

  it('nao regride para "1000k" logo abaixo de 1M', () => {
    // O corte tem que olhar o valor JA ARREDONDADO: testando o bruto em >= 1e6, 999_700
    // arredondava para "1000k" — exatamente o defeito que esta funcao existe pra evitar.
    expect(ctxWindow(999_700)).toBe('1M');
    expect(ctxWindow(999_499)).toBe('999k');
  });
});

describe('fileKind', () => {
  it('deriva o tipo da extensao, ignorando caixa', () => {
    expect(fileKind('1234-ab.PNG')).toBe('image');
    expect(fileKind('clip.mp4')).toBe('video');
    expect(fileKind('doc.pdf')).toBe('pdf');
  });

  it('extensao sem preview -> null (chip generico na galeria)', () => {
    expect(fileKind('pacote.zip')).toBeNull();
    expect(fileKind('Makefile')).toBeNull();
  });
});

describe('fmtBytes', () => {
  it('usa unidade binaria e so mostra decimal a partir de MB', () => {
    expect(fmtBytes(512)).toBe('512 B');
    expect(fmtBytes(2048)).toBe('2 KB');
    expect(fmtBytes(1024 * 1024 * 1.5)).toBe('1.5 MB');
    expect(fmtBytes(1024 * 1024)).toBe('1 MB');   // sem "1.0 MB"
    expect(fmtBytes(3 * 1024 ** 3)).toBe('3 GB');
  });
});

describe('providerName', () => {
  // Regressão: o desktop escrevia `provider === 'codex' ? 'Codex' : 'Claude'`, então uma sessão Pi
  // era rotulada "Claude" no painel de contexto.
  it('names the third provider instead of falling back to Claude', () => {
    expect(providerName('pi')).toBe('Pi');
  });

  it('names the fourth provider (kimi) instead of falling back to Claude', () => {
    expect(providerName('kimi')).toBe('Kimi');
  });

  it('keeps Claude and Codex byte-identical', () => {
    expect(providerName('claude')).toBe('Claude');
    expect(providerName('codex')).toBe('Codex');
  });

  it('falls back to Claude when the field is absent (backend default)', () => {
    expect(providerName(undefined)).toBe('Claude');
    expect(providerName(null)).toBe('Claude');
  });
});

describe('providerTag', () => {
  it('marca só as sessões que NÃO são Claude', () => {
    expect(providerTag('pi')).toBe('Pi');
    expect(providerTag('codex')).toBe('Codex');
    expect(providerTag('kimi')).toBe('Kimi');
  });

  it('não marca Claude (maioria das linhas — chip em todas seria ruído)', () => {
    expect(providerTag('claude')).toBeNull();
    expect(providerTag(undefined)).toBeNull();
    expect(providerTag(null)).toBeNull();
  });

  it('provider desconhecido não vira um chip "Claude" mentiroso', () => {
    expect(providerTag('gemini' as any)).toBeNull();
  });
});

describe('untrackedReason', () => {
  it('Pi e Kimi: transcript tardio é normal antes do 1º turno', () => {
    expect(untrackedReason('pi')).toContain('1º turno');
    expect(untrackedReason('kimi')).toContain('1º turno');
    expect(untrackedReason('kimi')).toContain('Kimi');
  });

  it('Claude (e default): o problema é a falta de --session-id', () => {
    expect(untrackedReason('claude')).toContain('--session-id');
    expect(untrackedReason(undefined)).toContain('--session-id');
  });
});

describe('summarizeText', () => {
  it('não parte emoji no meio ao truncar', () => {
    // slice() corta por unidade UTF-16 e deixava meio emoji órfão antes do "…".
    expect(summarizeText('😀abc', 2)).toBe('😀…');
    expect(Array.from(summarizeText('a😀b😀c😀d', 4)).length).toBe(4);
  });

  it('deixa passar o que cabe no limite', () => {
    expect(summarizeText('abc', 5)).toBe('abc');
    expect(summarizeText('abcde', 5)).toBe('abcde');   // fronteira: == max fica inteiro
  });

  it('corta em max chars CONTANDO o reticencia', () => {
    expect(summarizeText('abcdef', 5)).toBe('abcd…');
    expect(summarizeText('abcdef', 5)).toHaveLength(5);
  });

  it('achata quebra de linha num espaco (a linha do chat e nowrap)', () => {
    expect(summarizeText('foo\n  bar\tbaz  ')).toBe('foo bar baz');
  });
});

describe('summarizeToolInput', () => {
  it('mostra o VALOR cru, sem prefixo de chave', () => {
    expect(summarizeToolInput('Read', { file_path: '/tmp/x.ts' })).toBe('/tmp/x.ts');
    expect(summarizeToolInput('Write', { path: '/tmp/y.ts' })).toBe('/tmp/y.ts');
    expect(summarizeToolInput('Bash', { command: 'npm test' })).toBe('npm test');
    expect(summarizeToolInput('WebSearch', { query: 'svelte 5 runes' })).toBe('svelte 5 runes');
    expect(summarizeToolInput('WebFetch', { url: 'https://x.dev' })).toBe('https://x.dev');
  });

  it('Read anexa o recorte lido entre parenteses', () => {
    expect(summarizeToolInput('Read', { file_path: '/a.ts', limit: 500 })).toBe('/a.ts (limit=500)');
    expect(summarizeToolInput('Read', { file_path: '/a.ts', offset: 10, limit: 20 })).toBe('/a.ts (offset=10, limit=20)');
    expect(summarizeToolInput('Read', { file_path: '/a.ts', offset: 0 })).toBe('/a.ts');   // 0 nao e recorte
  });

  it('AskUserQuestion mostra a pergunta, nunca [object Object]', () => {
    // `questions` e lista de OBJETOS: no fallback generico o card saia "AskUserQuestion
    // [object Object]" (visto numa sessao Kimi). As opcoes ficam com o stepper, que abre por cima.
    const input = { questions: [{ question: 'O README deve ter 1 ou 2 linhas?', header: 'README',
                                  options: [{ label: '1 linha' }, { label: '2 linhas' }] }] };
    expect(summarizeToolInput('AskUserQuestion', input)).toBe('O README deve ter 1 ou 2 linhas?');
    expect(summarizeToolInput('AskUserQuestion', { questions: [] })).toBe('');
    // Forma inesperada vira linha vazia, nunca o mesmo "[object Object]" um nivel mais fundo.
    expect(summarizeToolInput('AskUserQuestion', { questions: [{ question: { t: 'x' } }] })).toBe('');
    expect(summarizeToolInput('AskUserQuestion', { questions: ['cru'] })).toBe('');
    expect(summarizeToolInput('AskUserQuestion', { questions: 'nao e lista' })).toBe('');
  });

  it('Grep/Glob mostram o padrao entre aspas (nao o diretorio), com o onde depois', () => {
    expect(summarizeToolInput('Grep', { pattern: 'foo', path: '/tmp' })).toBe('"foo" em /tmp');
    expect(summarizeToolInput('Glob', { pattern: '**/*.ts' })).toBe('"**/*.ts"');
  });

  it('corta valor unico em 72 chars', () => {
    const out = summarizeToolInput('Bash', { command: 'x'.repeat(200) });
    expect(out).toBe(`${'x'.repeat(71)}\u2026`);
    expect(out).toHaveLength(72);
  });

  it('varias queries: 1a em 48 + contador mudo', () => {
    expect(summarizeToolInput('WebSearch', { queries: ['a', 'b', 'c'] })).toBe('a (+2 consultas)');
    const long = 'y'.repeat(100);
    expect(summarizeToolInput('WebSearch', { queries: [long, 'b'] })).toBe(`${'y'.repeat(47)}\u2026 (+1 consultas)`);
  });

  it('uma query so na lista usa o limite cheio de 72', () => {
    expect(summarizeToolInput('WebSearch', { queries: ['z'.repeat(100)] })).toBe(`${'z'.repeat(71)}\u2026`);
  });

  it('escolhe a chave saliente, nao a 1a do objeto', () => {
    expect(summarizeToolInput('Task', { subagent_type: 'x', description: 'mapear a UI' })).toBe('mapear a UI');
  });

  it('sem input / sem valor -> sem resumo', () => {
    expect(summarizeToolInput('Bash', null)).toBe('');
    expect(summarizeToolInput('Bash', {})).toBe('');
    expect(summarizeToolInput('Read', { file_path: '' })).toBe('');
  });
});

describe('toolPhase', () => {
  it('sem tool_result ainda -> rodando', () => {
    expect(toolPhase(null)).toBe('pending');
    expect(toolPhase(undefined)).toBe('pending');
  });
  it('resultado normal -> concluido; is_error -> erro', () => {
    expect(toolPhase({})).toBe('done');
    expect(toolPhase({ is_error: false })).toBe('done');
    expect(toolPhase({ is_error: true })).toBe('error');
  });
});

describe('summarizeToolResult', () => {
  it('a frase muda por ferramenta (mesmo vocabulario do pacote)', () => {
    const tres = 'a\nb\nc';
    expect(summarizeToolResult({ result: tres }, 'Bash')).toBe('Pronto (3 linhas)');
    expect(summarizeToolResult({ result: tres }, 'Read')).toBe('3 linhas carregadas');
    expect(summarizeToolResult({ result: tres }, 'Grep')).toBe('3 linhas retornadas');
    expect(summarizeToolResult({ result: tres })).toBe('3 linhas retornadas');
  });

  it('singular em cada frase', () => {
    expect(summarizeToolResult({ result: 'so uma' }, 'Bash')).toBe('Pronto (1 linha)');
    expect(summarizeToolResult({ result: 'so uma' }, 'Read')).toBe('1 linha carregada');
    expect(summarizeToolResult({ result: 'so uma' })).toBe('1 linha retornada');
  });

  it('erro mostra a PRIMEIRA LINHA do erro no lugar da contagem', () => {
    expect(summarizeToolResult({ result: 'File does not exist.\nstack\nstack', is_error: true }, 'Read'))
      .toBe('File does not exist.');
  });

  it('erro longo tambem respeita o limite de 72', () => {
    expect(summarizeToolResult({ result: 'e'.repeat(100), is_error: true })).toBe(`${'e'.repeat(71)}\u2026`);
  });

  it('erro sem texto ainda diz que falhou', () => {
    expect(summarizeToolResult({ result: '  ', is_error: true })).toBe('Falhou');
  });

  it('resultado vazio (comando mudo) -> Pronto; ainda rodando -> nada', () => {
    expect(summarizeToolResult({ result: '   \n ' }, 'Bash')).toBe('Pronto');
    expect(summarizeToolResult(null)).toBe('');
    expect(summarizeToolResult(undefined)).toBe('');
  });
});

describe('toolGroupLabel', () => {
  it('todas do mesmo tipo -> o nome delas', () => {
    expect(toolGroupLabel(['Read', 'Read', 'Read'])).toBe('Read');
  });
  it('misturadas -> rotulo generico', () => {
    expect(toolGroupLabel(['Read', 'Bash'])).toBe('Ferramentas');
  });
  it('sem nome -> nao quebra', () => {
    expect(toolGroupLabel([null, null])).toBe('Tool');
    expect(toolGroupLabel([null, 'Read'])).toBe('Ferramentas');
  });
});

describe('toolGroupCounts', () => {
  it('so concluidas', () => {
    expect(toolGroupCounts(['done', 'done', 'done'])).toBe('3 concluídos');
    expect(toolGroupCounts(['done'])).toBe('1 concluído');
  });
  it('mistura na ordem rodando -> ok -> erro', () => {
    expect(toolGroupCounts(['done', 'error', 'pending', 'done'])).toBe('1 rodando • 2 concluídos • 1 com erro');
  });
  it('lista vazia -> nada', () => {
    expect(toolGroupCounts([])).toBe('');
  });
});

describe('splitTodoBlock', () => {
  const panel = [
    'Todos (11/13)',
    '├─ ✓ Avisar back: revisar DDL',
    '├─ ◐ Virando o store pro BFF',
    '└─ +3 more (3 completed)',
  ].join('\n');

  it('separa cabeçalho, árvore e o resto', () => {
    const out = splitTodoBlock(`Antes\n\n${panel}\n\nDepois`)!;
    expect(out.head).toBe('Todos (11/13)');
    expect(out.body.split('\n')).toHaveLength(3);
    expect(out.rest).toBe('Antes\n\n\nDepois');
  });

  it('prosa sem painel não casa', () => {
    expect(splitTodoBlock('Fechei os Todos (11/13) hoje.')).toBeNull();
  });

  it('cabeçalho sem árvore não casa (pode ser prosa)', () => {
    expect(splitTodoBlock('Todos (11/13)\nsegue o baile')).toBeNull();
  });

  // Painel do KIMI (0.37.2, medido em screenshots de 19/08/2026): cabeçalho "Todo" seco, sem
  // contador, e itens com o glifo de status ✓/●/○ — nada de árvore box-drawing.
  const panelKimi = [
    'Todo',
    '✓ Feature: troca de modelo Kimi (commit 3635b559)',
    '● Chip mandar agora não some',
    '○ Popover transparente demais',
  ].join('\n');

  it('formato kimi: separa cabeçalho, itens e o resto', () => {
    const out = splitTodoBlock(`Escrevendo antes\n${panelKimi}`)!;
    expect(out.head).toBe('Todo');
    expect(out.body.split('\n')).toHaveLength(3);
    expect(out.rest).toBe('Escrevendo antes');
  });

  it('formato kimi: "Todo" sozinho não casa (pode ser prosa)', () => {
    expect(splitTodoBlock('Todo\nna verdade era outra coisa')).toBeNull();
  });
});

describe('parseImageMessage', () => {
  const cap = 'olha esse bug ai';
  it('lê o formato que o app digita (uma linha, N paths)', () => {
    const out = parseImageMessage(`${cap} — 📎 imagem: /up/a.png 📎 imagem: /up/b.png`)!;
    expect(out.caption).toBe(cap);
    expect(out.filenames).toEqual(['a.png', 'b.png']);
  });
  it('tira o basename de caminho do WINDOWS (só barra invertida)', () => {
    // O marcador carrega o caminho NATIVO da sessão. No Windows ele não tem `/` nenhum, e o split
    // só por `/` devolvia o caminho inteiro como basename: a URL virava
    // `/uploads/C%3A%5C…`, o backend respondia 400 e a foto aparecia quebrada no celular.
    const out = parseImageMessage(
      `${cap} — 📎 imagem: C:\\cockpit\\.hangar-uploads\\1787356601-230c76.png`,
    )!;
    expect(out.caption).toBe(cap);
    expect(out.filenames).toEqual(['1787356601-230c76.png']);
  });
  it('lê o formato REESCRITO pelo Claude Code (prefixo, quebra de linha, último path consumido)', () => {
    // Sem isto a bolha que SOBRA no chat (a do transcript) mostrava os caminhos em texto cru --
    // era a metade feia do par duplicado que o usuário viu em 03/08/2026.
    const out = parseImageMessage(`[Image #1]${cap} — 📎 imagem:\n/up/a.png 📎 imagem:`)!;
    expect(out.caption).toBe(cap);
    expect(out.filenames).toEqual(['a.png']);
  });
  it('uma foto só, path consumido pelo agente: legenda limpa e lista vazia', () => {
    // O caso MAIS comum do celular: uma foto. O Claude Code a absorve como anexo real e apaga o
    // path, deixando o marcador pendurado. Devolver null aqui jogava a bolha pro texto cru.
    const out = parseImageMessage(`[Image #1]${cap} — 📎 imagem:`)!;
    expect(out.caption).toBe(cap);
    expect(out.filenames).toEqual([]);
  });
  it('sem marcador não é mensagem de imagem', () => {
    expect(parseImageMessage('só texto')).toBeNull();
  });
});

describe('formatacao segue o idioma', () => {
  // Troca o locale pelo MESMO caminho que a tela usa: localeAtual() chama getLocale() do
  // Paraglide, e overwriteGetLocale e o hook oficial do runtime pra substituir a resolucao em
  // teste (a tela escolhe via setLocale, mas em teste nao ha reload). As mensagens m.* leem
  // getLocale() a cada chamada, entao o hook cobre as duas pontas.
  it('relativeTime devolve "agora" em pt e "now" em en', () => {
    overwriteGetLocale(() => 'pt');
    expect(relativeTime(Date.now() / 1000)).toBe('agora');
    overwriteGetLocale(() => 'en');
    expect(relativeTime(Date.now() / 1000)).toBe('now');
  });

  it('intlLocale devolve etiqueta BCP-47 com regiao', () => {
    overwriteGetLocale(() => 'pt');
    expect(intlLocale()).toBe('pt-BR');
    overwriteGetLocale(() => 'en');
    expect(intlLocale()).toBe('en-US');
  });

  it('rotuloEstado cobre os quatro estados nos dois idiomas', () => {
    for (const st of ['working', 'idle', 'awaiting_input', 'dead'] as const) {
      overwriteGetLocale(() => 'pt');
      expect(rotuloEstado(st)).not.toBe('');
      overwriteGetLocale(() => 'en');
      expect(rotuloEstado(st)).not.toBe('');
    }
  });
});

describe('parseCanal', () => {
  it('tira o rótulo do começo e devolve o corpo sem ele', () => {
    expect(parseCanal('[vigia] ARMADA sobre: x')).toEqual({ canal: 'vigia', text: 'ARMADA sobre: x' });
  });

  it('link markdown no começo NÃO é canal (o rótulo do link se perderia)', () => {
    expect(parseCanal('[log](https://ex.com/a) caiu de novo')).toBeNull();
  });

  it('colchete no meio da frase e rótulo longo demais não viram etiqueta', () => {
    expect(parseCanal('olha isso [um aparte] aqui')).toBeNull();
    expect(parseCanal('[rotulo-comprido-demais-pra-etiqueta] x')).toBeNull();
  });
});

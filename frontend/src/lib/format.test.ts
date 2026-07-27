import { describe, it, expect } from 'vitest';
import {
  abbrevNum, attentionFeed, countAwaiting, effectiveGroupBy, fmtWhen, groupSelectedByServer, initials, nextAwaiting,
  projectKey, projectLabel, encodeCompareIds, parseCompareIds, latestAssistantEvent, resetsIn,
  clusterByPair, sortSessions, bubblesFromTail, ctxWindow, fileKind, fmtBytes, providerName,
  summarizeText, summarizeToolInput, summarizeToolResult,
} from './format';
import type { ChatEvent, State } from './types';

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
  it('takes first letter of each of two words', () => {
    expect(initials('claude-pocket')).toBe('CP');
  });
  it('splits on non-alphanumeric separators', () => {
    expect(initials('my-org_web')).toBe('PW');
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
  it('forces project with a single server (server grouping would be one giant group)', () => {
    expect(effectiveGroupBy('server', 1)).toBe('project');
    expect(effectiveGroupBy('project', 1)).toBe('project');
  });
  it('forces project with zero servers too', () => {
    expect(effectiveGroupBy('server', 0)).toBe('project');
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
    const expected = new Date(ts * 1000).toLocaleString([], { dateStyle: 'short', timeStyle: 'short' });
    expect(fmtWhen(ts)).toBe(expected);
    expect(fmtWhen(ts)).not.toBe('');
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

  it('keeps Claude and Codex byte-identical', () => {
    expect(providerName('claude')).toBe('Claude');
    expect(providerName('codex')).toBe('Codex');
  });

  it('falls back to Claude when the field is absent (backend default)', () => {
    expect(providerName(undefined)).toBe('Claude');
    expect(providerName(null)).toBe('Claude');
  });
});

describe('summarizeText', () => {
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
  it('mantem os resumos que ja existiam', () => {
    expect(summarizeToolInput('Read', { file_path: '/tmp/x.ts' })).toBe('path: /tmp/x.ts');
    expect(summarizeToolInput('Write', { path: '/tmp/y.ts' })).toBe('path: /tmp/y.ts');
    expect(summarizeToolInput('Bash', { command: 'npm test' })).toBe('cmd: npm test');
    expect(summarizeToolInput('WebSearch', { query: 'svelte 5 runes' })).toBe('query: svelte 5 runes');
    expect(summarizeToolInput('WebFetch', { url: 'https://x.dev' })).toBe('url: https://x.dev');
    expect(summarizeToolInput('Grep', { pattern: 'foo' })).toBe('pattern: foo');
  });

  it('Grep/Glob mostram o padrao (nao o diretorio), com o onde depois', () => {
    expect(summarizeToolInput('Grep', { pattern: 'foo', path: '/tmp' })).toBe('pattern: foo em /tmp');
    expect(summarizeToolInput('Glob', { pattern: '**/*.ts' })).toBe('pattern: **/*.ts');
  });

  it('corta valor unico em 72 chars', () => {
    const cmd = 'x'.repeat(200);
    const out = summarizeToolInput('Bash', { command: cmd });
    expect(out).toBe(`cmd: ${'x'.repeat(71)}…`);
    expect(out.slice('cmd: '.length)).toHaveLength(72);
  });

  it('varias queries: 1a em 48 + contador mudo', () => {
    expect(summarizeToolInput('WebSearch', { queries: ['a', 'b', 'c'] })).toBe('query: a (+2 consultas)');
    const long = 'y'.repeat(100);
    expect(summarizeToolInput('WebSearch', { queries: [long, 'b'] })).toBe(`query: ${'y'.repeat(47)}… (+1 consultas)`);
  });

  it('uma query so na lista usa o limite cheio de 72', () => {
    const long = 'z'.repeat(100);
    expect(summarizeToolInput('WebSearch', { queries: [long] })).toBe(`query: ${'z'.repeat(71)}…`);
  });

  it('escolhe a chave saliente, nao a 1a do objeto', () => {
    expect(summarizeToolInput('Task', { subagent_type: 'x', description: 'mapear a UI' })).toBe('description: mapear a UI');
  });

  it('sem input / sem valor -> sem resumo', () => {
    expect(summarizeToolInput('Bash', null)).toBe('');
    expect(summarizeToolInput('Bash', {})).toBe('');
    expect(summarizeToolInput('Read', { file_path: '' })).toBe('');
  });
});

describe('summarizeToolResult', () => {
  it('conta as linhas, com singular', () => {
    expect(summarizeToolResult({ result: 'so uma' })).toBe('1 linha');
    expect(summarizeToolResult({ result: Array.from({ length: 40 }, (_, i) => `l${i}`).join('\n') })).toBe('40 linhas');
  });

  it('erro mostra a PRIMEIRA LINHA do erro no lugar da contagem', () => {
    expect(summarizeToolResult({ result: 'File does not exist.\nstack\nstack', is_error: true }))
      .toBe('File does not exist.');
  });

  it('erro longo tambem respeita o limite de 72', () => {
    expect(summarizeToolResult({ result: 'e'.repeat(100), is_error: true })).toBe(`${'e'.repeat(71)}…`);
  });

  it('ainda rodando / resultado vazio -> nada na linha', () => {
    expect(summarizeToolResult(null)).toBe('');
    expect(summarizeToolResult({ result: '   \n ' })).toBe('');
  });
});

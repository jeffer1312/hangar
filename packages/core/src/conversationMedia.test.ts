import { describe, expect, it } from 'vitest';
import {
  attachmentFolder, conversationAttachments, conversationMedia, filterByOrigin, groupByDay, localDayStart,
  messageAttachments, newestFirst, userMessageFiles,
} from './conversationMedia';
import type { ChatEvent } from './types';

const ev = (kind: ChatEvent['kind'], id: string, text: string, ts: number | null = 100, extra: Partial<ChatEvent> = {}): ChatEvent =>
  ({ kind, id, text, ts, ...extra });

describe('messageAttachments', () => {
  it('assistant: cited paths first, then remote media, keys follow the bubble order', () => {
    const a = messageAttachments(ev('assistant_msg', 'a1',
      'see /tmp/shot.png and /tmp/report.pdf, plus https://cdn.test/clip.mp4'));
    expect(a.map((x) => [x.key, x.source, x.kind, x.name, x.ref, x.origin])).toEqual([
      ['a1#0', 'cited', 'image', 'shot.png', '/tmp/shot.png', 'agent'],
      ['a1#1', 'cited', 'pdf', 'report.pdf', '/tmp/report.pdf', 'agent'],
      ['a1#2', 'remote', 'video', 'clip.mp4', 'https://cdn.test/clip.mp4', 'agent'],
    ]);
  });

  it('user: videos, photos, terminal images, then other files', () => {
    // The last photo lost its path (absorbed as a real attachment): it comes from the transcript.
    const text = 'look — 📎 arquivo: /up/v.mp4 📎 imagem: /up/a.png 📎 arquivo: /up/notes.docx 📎 imagem:';
    const a = messageAttachments(ev('user_msg', 'u1', text, 5, { image_count: 1 }));
    expect(a.map((x) => [x.key, x.source, x.kind, x.ref])).toEqual([
      ['u1#0', 'upload', 'video', 'v.mp4'],
      ['u1#1', 'upload', 'image', 'a.png'],
      ['u1#2', 'transcript', 'image', '0'],
      ['u1#3', 'upload', 'document', 'notes.docx'],
    ]);
    expect(a.every((x) => x.origin === 'user' && x.ts === 5)).toBe(true);
  });

  it('user: a photo written AND pasted is counted once', () => {
    const f = userMessageFiles('📎 imagem: /up/a.png 📎 imagem: /up/b.png', 1);
    expect(f).toEqual({ videos: [], images: ['a.png'], transcript: 1, files: [] });
  });

  it('queued and held echoes stay out', () => {
    for (const e of [
      ev('user_msg', 'queued-1', '📎 imagem: /up/a.png'),
      ev('user_msg', 'held:2', '📎 imagem: /up/a.png'),
      ev('user_msg', 'u3', '📎 imagem: /up/a.png', 1, { desistiu: true }),
    ]) expect(messageAttachments(e)).toEqual([]);
  });

  it('plain text and tool events have nothing', () => {
    expect(messageAttachments(ev('user_msg', 'u', 'hello'))).toEqual([]);
    expect(messageAttachments(ev('tool_use', 't', '/tmp/a.png'))).toEqual([]);
  });
});

describe('conversationMedia', () => {
  it('keeps only images and videos, oldest first', () => {
    const events = [
      ev('user_msg', 'u1', '📎 imagem: /up/a.png', 1),
      ev('assistant_msg', 'a1', 'wrote /tmp/r.pdf and /tmp/b.png', 2),
      ev('assistant_msg', 'a2', 'audio /tmp/n.mp3', 3),
    ];
    expect(conversationMedia(events).map((x) => x.key)).toEqual(['u1#0', 'a1#1']);
    expect(conversationAttachments(events).map((x) => x.kind)).toEqual(['image', 'pdf', 'image', 'audio']);
  });
});

describe('filters and grouping', () => {
  const items = [
    { id: 1, origin: 'agent' as const, ts: new Date(2026, 10, 21, 11, 42).getTime() / 1000 },
    { id: 2, origin: 'user' as const, ts: new Date(2026, 10, 21, 9, 30).getTime() / 1000 },
    { id: 3, origin: 'agent' as const, ts: new Date(2026, 10, 20, 22, 5).getTime() / 1000 },
    { id: 4, origin: 'user' as const, ts: null },
  ];

  it('filterByOrigin', () => {
    expect(filterByOrigin(items, 'all').map((x) => x.id)).toEqual([1, 2, 3, 4]);
    expect(filterByOrigin(items, 'agent').map((x) => x.id)).toEqual([1, 3]);
    expect(filterByOrigin(items, 'user').map((x) => x.id)).toEqual([2, 4]);
  });

  it('groupByDay keeps arrival order and puts undated last', () => {
    const g = groupByDay(items, (x) => x.ts);
    expect(g.map((x) => x.items.map((i) => i.id))).toEqual([[1, 2], [3], [4]]);
    expect(g[0].day).toBe(localDayStart(items[0].ts!));
    expect(g[1].day).toBe(new Date(2026, 10, 20).getTime() / 1000);
    expect(g[2].day).toBeNull();
  });

  it('newestFirst sorts by time and keeps ties stable', () => {
    const tie = [{ id: 'a', ts: 5 }, { id: 'b', ts: null }, { id: 'c', ts: 9 }, { id: 'd', ts: 5 }];
    expect(newestFirst(tie, (x) => x.ts).map((x) => x.id)).toEqual(['c', 'a', 'd', 'b']);
  });
});

describe('attachmentFolder', () => {
  it.each([
    ['/home/jefferson/Projetos/hangar/out/r.pdf', '~/Projetos/hangar/out/'],
    ['/Users/ana/x/y.zip', '~/x/'],
    ['/tmp/a.csv', '/tmp/'],
    ['/a.csv', '/'],
    ['out/r.pdf', 'out/'],
    ['C:\\work\\out\\r.pdf', 'C:/work/out/'],
    ['r.pdf', ''],
  ])('%s → %s', (path, folder) => {
    expect(attachmentFolder(path)).toBe(folder);
  });
});

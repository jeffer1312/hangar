import { parsePeerMessage } from '@hangar/core';
import type { ChatEvent } from '@hangar/core';

export interface PeerMsg {
  from: string;
  to: string;
  text: string;
  ts: number;
}

export interface PairHistory {
  ok: boolean;
  h: ChatEvent[];
}

export interface PairFeed {
  feed: PeerMsg[];
  failed: string[];
}

export function montarFeed(all: string[], historicos: PairHistory[]): PairFeed {
  const members = new Set(all);
  const failed = all.filter((_, index) => !historicos[index]?.ok);
  const feed: PeerMsg[] = [];

  historicos.forEach((history, index) => {
    const owner = all[index];
    if (!owner || !history?.ok) return;

    for (const event of history.h) {
      if (event.kind !== 'user_msg' || !event.text) continue;
      const peer = parsePeerMessage(event.text);
      if (peer && peer.from !== owner && members.has(peer.from)) {
        feed.push({ from: peer.from, to: owner, text: peer.text, ts: event.ts ?? 0 });
      }
    }
  });

  feed.sort((a, b) => a.ts - b.ts);
  return { feed: feed.slice(-40), failed };
}

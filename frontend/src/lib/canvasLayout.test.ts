import { describe, expect, it } from 'vitest';
import { placeNew, resizeBox, MIN_W, MIN_H, CARD_W, CARD_H, GAP, PAD, type CanvasLayout } from './canvasLayout';

const row = (key: string, serverId: string, pairGid: string | null = null) => ({ key, serverId, pairGid });

describe('placeNew', () => {
  it('canvas vazio: primeiro card no PAD, servidor seguinte na coluna seguinte', () => {
    const out = placeNew({}, [row('a::x', 'a'), row('b::y', 'b')], ['a', 'b']);
    expect(out['a::x']).toEqual({ x: PAD, y: PAD, w: CARD_W, h: CARD_H });
    expect(out['b::y'].x).toBe(PAD + CARD_W + GAP);
    expect(out['b::y'].y).toBe(PAD);
  });

  it('empilha abaixo do card mais fundo que intersecta a coluna', () => {
    const existing: CanvasLayout = { 'a::x': { x: PAD, y: PAD, w: CARD_W, h: 400 } };
    const out = placeNew(existing, [row('a::z', 'a')], ['a']);
    expect(out['a::z'].y).toBe(PAD + 400 + GAP);
    expect(out['a::z'].x).toBe(PAD);
  });

  it('não devolve chaves que já têm posição', () => {
    const existing: CanvasLayout = { 'a::x': { x: 10, y: 10, w: 300, h: 200 } };
    expect(placeNew(existing, [row('a::x', 'a')], ['a'])).toEqual({});
  });

  it('pareados do mesmo servidor nascem consecutivos (mesmo com intrusos no meio)', () => {
    const out = placeNew({}, [row('a::p1', 'a', 'g1'), row('a::solo', 'a'), row('a::p2', 'a', 'g1')], ['a']);
    expect(out['a::p2'].y).toBe(out['a::p1'].y + CARD_H + GAP);      // p2 logo abaixo de p1
    expect(out['a::solo'].y).toBe(out['a::p2'].y + CARD_H + GAP);    // solo depois do grupo
  });

  it('card arrastado pra dentro da coluna conta pro fundo dela', () => {
    const existing: CanvasLayout = { 'x': { x: PAD + 50, y: 600, w: CARD_W, h: 100 } }; // sobrepõe a coluna 0
    const out = placeNew(existing, [row('a::n', 'a')], ['a']);
    expect(out['a::n'].y).toBe(600 + 100 + GAP);
  });

  it('servidor desconhecido em serverOrder cai na coluna 0 (defensivo)', () => {
    const out = placeNew({}, [row('z::n', 'z')], []);
    expect(out['z::n'].x).toBe(PAD);
  });

  it('adjacência exata (borda direita em PAD) não conta como interseção', () => {
    // Box à esquerda com x+w === PAD: encosta na coluna 0 mas não a sobrepõe -> não empurra o card novo.
    const existing: CanvasLayout = { 'x': { x: PAD - CARD_W, y: 600, w: CARD_W, h: 200 } };
    const out = placeNew(existing, [row('a::n', 'a')], ['a']);
    expect(out['a::n'].y).toBe(PAD);
  });

  it('coluna B cheia não muda o y de um card novo na coluna A', () => {
    const bx = PAD + CARD_W + GAP;   // x da coluna do servidor b
    const existing: CanvasLayout = { 'b::deep': { x: bx, y: 900, w: CARD_W, h: 300 } };
    const out = placeNew(existing, [row('a::n', 'a')], ['a', 'b']);
    expect(out['a::n'].x).toBe(PAD);
    expect(out['a::n'].y).toBe(PAD);
  });
});

describe('resizeBox', () => {
  const b = { x: 100, y: 100, w: 400, h: 300 };

  it('leste/sul só crescem w/h, x/y ficam', () => {
    expect(resizeBox(b, 'se', 50, 40)).toEqual({ x: 100, y: 100, w: 450, h: 340 });
  });

  it('oeste/norte movem x/y junto com o tamanho', () => {
    expect(resizeBox(b, 'nw', 60, 30)).toEqual({ x: 160, y: 130, w: 340, h: 270 });
  });

  it('clampa no mínimo sem deslocar a borda oposta', () => {
    const out = resizeBox(b, 'nw', 9999, 9999);
    expect(out.w).toBe(MIN_W);
    expect(out.h).toBe(MIN_H);
    expect(out.x + out.w).toBe(b.x + b.w);   // borda leste parada
    expect(out.y + out.h).toBe(b.y + b.h);   // borda sul parada
  });

  it('borda oeste/norte não passa de 0 (card sairia do plano)', () => {
    const out = resizeBox(b, 'nw', -9999, -9999);
    expect(out.x).toBe(0);
    expect(out.y).toBe(0);
    expect(out.w).toBe(500);   // x0 + w0
    expect(out.h).toBe(400);
  });
});

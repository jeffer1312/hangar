import { gcm } from '@noble/ciphers/aes.js';
import { hkdf } from '@noble/hashes/hkdf.js';
import { pbkdf2Async } from '@noble/hashes/pbkdf2.js';
import { sha256 } from '@noble/hashes/sha2.js';

// Par do `deriveKeys` de frontend/src/lib/sync.ts: as duas contas têm de dar o mesmo resultado, ou o
// login recusa a senha certa. O Hermes não tem `crypto.subtle`, e base64 e UTF-8 ficam aqui porque
// `atob`, `btoa` e `TextDecoder` não são garantidos em todo motor.
const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';

export function fromBase64(text: string): Uint8Array {
  const bytes: number[] = [];
  let buffer = 0;
  let bits = 0;
  for (const char of text.replace(/=+$/, '')) {
    buffer = (buffer << 6) | ALPHABET.indexOf(char);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((buffer >> bits) & 255);
      buffer &= (1 << bits) - 1;
    }
  }
  return Uint8Array.from(bytes);
}

export function toBase64(bytes: Uint8Array): string {
  let text = '';
  for (let i = 0; i < bytes.length; i += 3) {
    const left = bytes.length - i;
    const chunk = (bytes[i] << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    text += ALPHABET[chunk >> 18] + ALPHABET[(chunk >> 12) & 63]
      + (left > 1 ? ALPHABET[(chunk >> 6) & 63] : '=') + (left > 2 ? ALPHABET[chunk & 63] : '=');
  }
  return text;
}

// `encodeURIComponent` recusa surrogate sozinho; o `TextEncoder` da web o troca por U+FFFD.
const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g;
const utf8 = (text: string) => Uint8Array.from(
  (encodeURIComponent(text.replace(LONE_SURROGATE, '�')).match(/%..|./g) ?? [])
    .map((c) => (c.length > 1 ? parseInt(c.slice(1), 16) : c.charCodeAt(0))),
);
const fromUtf8 = (bytes: Uint8Array) =>
  decodeURIComponent(Array.from(bytes, (b) => `%${b.toString(16).padStart(2, '0')}`).join(''));

export async function deriveKeys(password: string, salt: string, iterations: number): Promise<{ authHash: string; encKey: Uint8Array }> {
  // Fatia longa: cada devolução ao laço de eventos custa um timer, e a conta já é demorada no Hermes.
  const master = await pbkdf2Async(sha256, utf8(password), fromBase64(salt), { c: iterations, dkLen: 32, asyncTick: 100 });
  const branch = (info: string) => hkdf(sha256, master, new Uint8Array(0), utf8(info), 32);
  return { authHash: toBase64(branch('cp-auth')), encKey: branch('cp-enc') };
}

export function decryptJson<T>(encKey: Uint8Array, blob: { iv: string; data: string }): T {
  return JSON.parse(fromUtf8(gcm(encKey, fromBase64(blob.iv)).decrypt(fromBase64(blob.data))));
}

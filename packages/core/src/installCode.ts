import { sha256Hex } from './hmac';

// Sem 0/O e 1/I: a pessoa compara de olho, numa tela e na outra.
const ALPHABET = '23456789ABCDEFGHJKLMNPQRSTUVWXYZ';

/** Código de conferência do celular, igual no celular e no cartão de quem aprova (o nativo repete em `connect.rs`).
 *  Sai do SHA-256 do `install_id`, não de um pedaço dele: quem pede escolhe o id e copiaria um prefixo.
 *  shortcut: são 30 bits; quem tem a senha da conta e viu o código ainda acha outro id com o mesmo código
 *  por força bruta (~2^30 hashes). Por isso o cartão mostra também o nome e há quanto tempo o pedido chegou. */
export function installCode(installId: string): string {
  const head = Number.parseInt(sha256Hex(new TextEncoder().encode(installId)).slice(0, 8), 16) >>> 2;
  const code = Array.from({ length: 6 }, (_, i) => ALPHABET[(head >>> (25 - 5 * i)) & 31]).join('');
  return `${code.slice(0, 3)}-${code.slice(3)}`;
}

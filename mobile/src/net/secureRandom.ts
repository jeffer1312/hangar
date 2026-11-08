// O Hermes não tem `crypto`, e o `expo-crypto` pediria binário novo; o `uuidv4` do Expo já está nele.
export function secureRandom(): string | null {
  const { expo } = globalThis as unknown as { expo?: { uuidv4?: () => string } };
  return expo?.uuidv4 ? `${expo.uuidv4()}${expo.uuidv4()}`.replace(/-/g, '') : null;
}

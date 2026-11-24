import { createContext } from 'react';

/** Abre a galeria da conversa na mídia desta chave (`<id do evento>#<posição>`); false = a chave não
 *  está na conversa carregada, e a miniatura abre só as mídias da própria mensagem. */
export const GalleryContext = createContext<((key: string) => boolean) | null>(null);

# Escape layer fix

## Status

Concluída.

Commit: `pending`

## Alterações

- `BottomSheet` agora trata Escape no próprio dialog e interrompe a propagação antes do fallback de `window`.
- O fallback de `window` também previne a propagação para handlers globais inferiores e continua fechando a sheet quando o foco está fora do conteúdo.
- O handler global do `Chat` ignora Escape originado dentro de qualquer `[role="dialog"]`, preservando o fechamento da camada superior sem fechar overlays rastreados atrás dela.

## Validação

- `npm run test`: 13 arquivos, 168 testes aprovados.
- `npm run check`: 0 erros, 0 avisos.
- `npm run build`: concluído; permanecem apenas avisos conhecidos do `lottie-web`/Vite.
- `git diff --check`: aprovado.

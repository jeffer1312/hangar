# Referência de anexos

Cada JSON contém a sequência HTTP completa (pedido, status, todos os cabeçalhos,
corpo em base64 e JSON decodificado) e a árvore completa do cofre (diretórios,
arquivos, tipo, modo, mtime, tamanho e bytes). Respostas do resolvedor de áudio e
de nomes hostis são identificadas pelo campo `kind`; não simulam respostas HTTP.

A referência é separada por plataforma. Erros herdados permanecem na captura,
sem alterar produção ou a expectativa dos testes existentes.

Somente estes voláteis são controlados:

- Relógio de anexos: `1700000000`; mtime dos arquivos é uma entrada explícita da
  fixture antes das consultas. Casos de retenção usam deslocamentos de dias inteiros.
- Aleatoriedade de nomes: contador hexadecimal reiniciado por fixture. Os nomes
  continuam sendo produzidos pela função real de gravação.
- Raiz absoluta: `<root>`; chave de projeto derivada da raiz: `<project>` ou
  `<other-project>`. As correspondências são consistentes entre resposta e árvore.
- Mtime de diretório: `<directory-mtime>`; não influencia retenção de arquivos.
- Corpo JSON: somente os trechos de strings que representam raízes e chaves de
  projeto são substituídos. Separadores de caminho após a raiz usam `/`. Espaços,
  ordem e duplicidade de campos, representação numérica e escapes fora desses
  trechos permanecem byte a byte, inclusive dentro de um token parcialmente alterado.
  A visão `json` é semântica; o `body_base64` conserva a representação recebida.
  O ajuste de `content-length` reflete somente essas substituições e conserva
  qualquer diferença original entre o comprimento anunciado e o corpo recebido.
  Corpos binários e HTML não são normalizados.

Os pedidos usam `Accept-Encoding: identity`. MIME, CSP, disposição de download,
Range, ETag, conteúdo, erros e campos nulos permanecem na referência. O cenário
`video-without-ffmpeg` exercita a ausência do decodificador sem skip. Transcrição
paga, autenticação e transferência de sessão ficam fora do contrato do harness.

A atualização é uma operação explícita, realizada somente no ambiente descartável
autorizado. A partir de `backend/`, com um diretório de dados de teste configurado:

```python
import os
from pathlib import Path
from tests.uploads_contract import FIXTURE_NAMES, UploadReference

reference = UploadReference(Path(os.environ["HANGAR_TEST_DATA_ROOT"]) / "goldens")
for name in FIXTURE_NAMES:
    reference.write_golden(name, approve=True)
```

O pytest só lê os arquivos; uma divergência nunca regrava a referência. A captura
`proxy-baseline.json` preserva o 503 real com 23 bytes entregues ao handler Python
bloqueado, sem PIDs nem dados de ambiente. O detector rejeita tanto o erro quanto
um sucesso que ainda repasse bytes; a implementação futura deve responder com
sucesso e zero bytes no upstream. Os scripts de medição, manifesto de ambiente
e observação detalhada do proxy ficam fora do Git.

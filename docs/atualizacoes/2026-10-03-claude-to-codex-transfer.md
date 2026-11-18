---
id: 2026-10-03-claude-to-codex-transfer
titulo: O seletor de contas permite continuar a sessão Claude no Codex
comando_posix: ./scripts/install-claude-wrapper.sh
comando_windows: powershell -ExecutionPolicy Bypass -File scripts/setup-windows-wrappers.ps1
prova: scripts/hangar-codex
destrutivo: false
---

Atualiza o lançador para receber `--tool-output-token-limit`. A configuração acompanha
abertura, reinício e retomada da sessão transferida; não muda o `config.toml` da conta.
Os instaladores já existentes publicam o lançador de forma idempotente nas duas plataformas.

Na retomada de uma conversa importada, escolhas omitidas vêm do registro da mesma thread:
conta, pasta, chave, permissões, modelo, esforço e orçamento. Registro incompleto não abre
um servidor com permissões padrão. A preparação usa a pasta do processo sem solicitar
promoção de confiança ao abrir a thread nativa.
Sobrescrever apenas uma opção de permissão exige que o par final corresponda a um dos três
modos disponíveis; combinações sem representação são recusadas antes de abrir o servidor.

A prova de existência acima é a exigida pelo atualizador. A aceitação do contrato exige
conferir `hangar-codex-tui --help` na instalação atualizada e capturar o conteúdo completo
no pedido nativo após retomada. Essas provas e Windows ficam para a validação autorizada.

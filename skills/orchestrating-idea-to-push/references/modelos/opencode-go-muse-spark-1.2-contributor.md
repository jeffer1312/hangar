# opencode-go/muse-spark-1.2-contributor — ficha do modelo

**Status: HIPÓTESE (varredura de 19/08/2026, ainda sem execução medida).** Modelo lançado em
05/08/2026 (Meta, família Muse Spark); a comunidade tem duas semanas de dados. Tudo abaixo vem do
fabricante e de reviews públicos, não de rodada nossa — a primeira execução real
(2026-08-19, enxugada/C5/permissão no hangar) deve substituir esta seção por medição.

## O que se sabe (fontes públicas, 19/08/2026)

- **Janela 1.0M de contexto, 131.1K de saída** (catálogo do Pi nesta máquina). Tools: sim.
- **Tier "contributor": prompts e completions viram dado de treino da Meta** — é o preço do
  desconto (12–21× mais barato que o tier normal). **Avisar o usuário antes de toda execução
  que use este tier** — o código do repo vai nos prompts. **NUNCA usar em código de
  cliente/Promédico.**
- **Teto de 60 requisições/minuto** no tier contributor (o tier normal tem 3.000) — executor
  único serial não deve sentir; fan-out de subagentes pode esbarrar.
- Posição em ranking público de coding: mediano (#22/135, ~62.8/100 numa agregação de 08/2026).
  Não é um modelo de topo: **plano conservador** — receita literal, régua numérica, Step curto.

## Hipóteses de trabalho (até a primeira medição)

- **Tratar como executor que aplica receita literal** (mesma postura da ficha do
  deepseek-v4-flash): investir no detalhe do Step; critério visual vira número.
- **Assumir que NÃO enxerga imagem** até prova em contrário → protocolo de visão do
  `executor.md` (`see <caminho>`) obrigatório nas Tasks de tela.
- Thinking levels reais: conferir com `/cp-think` na sessão viva (o Pi trunca pedido acima do
  teto do modelo sem erro — precedente k3/glm).

## Medições (preencher na retrospectiva da primeira execução)

- Rodadas até APROVA por tipo de Task: —
- Custo/dia na conta: — (perguntar a fatura ao usuário, nunca somar 💵)
- Enxerga imagem: —
- Segue receita literal vs improvisa: —

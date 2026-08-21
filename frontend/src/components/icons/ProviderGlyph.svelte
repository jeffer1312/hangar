<script lang="ts">
  // Glifo colorido do PROVIDER da sessão (Claude/Codex/Pi/Kimi) — referência do tab bar do
  // super.engineering, onde cada agente carrega a marca colorida dele e a aba se reconhece de
  // longe. Diferente do ProvedorIcone (contas/credenciais, deduzido da base_url), este é por
  // sessão: quem roda naquele pane.
  //
  // Traçados oficiais da coleção simple-icons (CC0-1.0), mesma fonte e mesma justificativa do
  // ProvedorIcone: CSP fechada + LAN = nada de CDN em runtime, e uma dependência npm pra quatro
  // traçados seria peso sem retorno. Claude e Kimi são os MESMOS paths de lá (não duplicar
  // desenho à mão). Codex usa a marca OpenAI. Pi não existe na coleção — cai no glifo "π"
  // tipográfico, como o próprio Oh My Pi se apresenta.
  //
  // Cor: a da marca, como no ProvedorIcone. A da OpenAI oficial (preto) some no tema escuro —
  // usamos o teal do ChatGPT (#10a37f), que lê nos dois temas e continua inequívoco.
  interface Props {
    // O domínio de sessão é Provider ('claude'|'codex'|'pi'|'kimi'), mas o de COTA traz
    // 'opencode' (e pode crescer) — por isso string: provider desconhecido cai na INICIAL,
    // como o ProvedorIcone faz com Groq, em vez de derrubar o componente pai (caiu uma vez:
    // conta OpenCode real quebrou o popover de cota com `'d' in undefined`).
    provider: string | null | undefined;
    size?: number;
  }
  let { provider, size = 13 }: Props = $props();

  type Marca = { d: string; cor: string } | { texto: string; cor: string };

  const MARCAS: Record<string, Marca> = {
    claude: { d: 'm4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z', cor: '#D97757' },
    codex: { d: 'M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927-.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z', cor: '#10a37f' },
    kimi: { d: 'M21.765.351C22.998.351 24 1.353 24 2.586S22.998 4.82 21.765 4.82h-1.974c-.15 0-.26-.12-.26-.26V2.586A2.237 2.237 0 0 1 21.765.35M9.41 13.388l8.447-8.377c.16-.16.07-.471-.14-.471h-4.55s-.1.02-.14.06l-9.099 9.029c-.14.14-.35.02-.35-.21V4.81c0-.15-.1-.27-.221-.27H.22c-.12 0-.22.12-.22.27v18.57c0 .15.1.27.22.27h3.137c.12 0 .22-.12.22-.27v-3.79c0-.08.03-.16.08-.21l2.826-2.796c.07-.07.16-.08.241-.03l7.546 5.551a8.9 8.9 0 0 0 4.018 1.493c.12.01.23-.11.23-.27V19.76c0-.14-.08-.25-.19-.26a5.8 5.8 0 0 1-2.355-.942l-6.533-4.73c-.14-.09-.15-.32-.03-.441', cor: '#c7bdf5' },
    pi: { texto: 'π', cor: '#8b5cf6' },
    // Domínio de cota (não de sessão): a marca OpenCode, mesma do ProvedorIcone.
    opencode: { d: 'M22 24H2V0h20zM17 4.8H7v14.4h10z', cor: '#e8e8e8' },
  };

  // provider ausente = Claude (mesma regra do providerName em lib/format: o campo só viaja quando
  // NÃO é Claude). Desconhecido = a INICIAL do nome, neutra — nunca quebra, nunca inventa marca.
  const marca = $derived<Marca>(
    MARCAS[provider ?? 'claude'] ?? { texto: (provider ?? '?').slice(0, 1).toUpperCase(), cor: '#8d8489' },
  );
</script>

<span class="pg" style="--pg-size:{size}px; --pg-cor:{marca.cor}" aria-hidden="true">
  {#if 'd' in marca}
    <svg viewBox="0 0 24 24" fill="currentColor"><path d={marca.d} /></svg>
  {:else}
    <span class="pg-txt">{marca.texto}</span>
  {/if}
</span>

<style>
  /* Quadrado em linha, fundo derivado da MESMA cor da marca (um valor por provider, como no
     ProvedorIcone) — no chip apertado da lista o glifo puro sem fundo se perdia no texto. */
  .pg {
    width: var(--pg-size);
    height: var(--pg-size);
    flex-shrink: 0;
    display: inline-grid;
    place-items: center;
    color: var(--pg-cor);
    background: color-mix(in srgb, var(--pg-cor) 16%, transparent);
    border-radius: 3.5px;
  }
  .pg :global(svg) { width: 72%; height: 72%; }
  /* O π é tipográfico: um degrau acima do glifo vetorial pra ocupar o mesmo peso visual. */
  .pg-txt { font-size: calc(var(--pg-size) * 0.82); font-weight: 700; line-height: 1; }
</style>

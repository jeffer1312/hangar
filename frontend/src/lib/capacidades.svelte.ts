// Capacidades do servidor ativo que o Chat le de GET /api/config e outros componentes consultam
// sem refazer o pedido. `null` = ainda nao lida (assume disponivel, como o terminal faz).
export const capacidades = $state<{ traducaoPensamento: boolean | null }>({ traducaoPensamento: null });

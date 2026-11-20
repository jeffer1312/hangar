import { deliveryModeDescription, deliveryModeLabel, deliveryOptions } from '../../chat/queueDelivery';
import { useDeliveryMode, useQueueDelivery } from '../../chat/useQueueDelivery';
import { useServers } from '../../stores/servers';
import { haptic } from '../../ui/haptics';
import { PickerRow } from '../dream/rows';
import * as m from '../../paraglide/messages';

// O modo é do aparelho; o Jev depende do servidor, e aqui vale o ativo.
export function DeliveryModeRow() {
  const server = useServers((s) => s.active());
  const delivery = useQueueDelivery(server ?? undefined, true);
  const options = deliveryOptions(delivery);
  const current = options.find((o) => o.checked)!;
  // Jev salvo e indisponível envia pela fila: a linha diz por quê, sem "…." dobrado.
  const description = delivery.mode === 'jev' && delivery.status !== 'ready'
    ? m.mobile_queue_jev_fallback({ reason: delivery.help.replace(/[\s.…]+$/, '') })
    : deliveryModeDescription(current.mode);
  const pick = (id: string) => {
    const option = options.find((o) => o.mode === id);
    if (!option || option.disabled || delivery.mode === option.mode) return;
    haptic.selection();
    useDeliveryMode.getState().setMode(option.mode);
  };
  return (
    <PickerRow title={m.delivery_mode_title()} value={deliveryModeLabel(current.mode)} description={description}
      options={options.map((o) => ({ id: o.mode, title: deliveryModeLabel(o.mode), selected: o.checked, disabled: o.disabled,
        subtitle: o.disabled ? delivery.help : deliveryModeDescription(o.mode) }))}
      onPick={pick} />
  );
}

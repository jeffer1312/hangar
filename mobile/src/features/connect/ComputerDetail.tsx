import { MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import * as m from '../../paraglide/messages';
/** O corpo do detalhe de um computador, só com dados: a folha e a miniatura da novidade desenham o mesmo. */
export function ComputerDetail({ online, inApp, next, busy, canEnter, removeDisabled, onOpen, onEnter, onRemove }: {
  online: boolean;
  /** Já está neste app e entra sem pedido. */
  inApp: boolean;
  /** A ação que cabe agora (entrar, tentar de novo), quando há uma. */
  next: { title: string; description?: string } | null;
  /** Remover da conta em andamento. */
  busy: boolean;
  canEnter: boolean;
  removeDisabled: boolean;
  onOpen: () => void;
  onEnter: () => void;
  onRemove: () => void;
}) {
  return <>
    <MachineGroup>
      <MachineRow title={m.connect_computer_state()} value={online ? m.connect_computer_online() : m.mobile_machines_offline()}
        valueTone={online ? 'success' : 'error'} />
      {inApp ? <MachineRow title={m.connect_computer_in_app()} selected /> : null}
    </MachineGroup>
    {inApp ? <MachineGroup>
      <MachineRow title={m.connect_computer_open()} action disabled={busy} onPress={onOpen} />
    </MachineGroup> : null}
    {next ? <MachineGroup>
      <MachineRow title={next.title} description={next.description} action disabled={!canEnter} onPress={onEnter} />
    </MachineGroup> : null}
    <MachineGroup>
      <MachineRow title={busy ? m.connect_computer_removing() : m.connect_computer_remove()} danger action
        disabled={removeDisabled} onPress={onRemove} />
    </MachineGroup>
    <MachineHelp>{m.connect_computer_remove_help()}</MachineHelp>
  </>;
}

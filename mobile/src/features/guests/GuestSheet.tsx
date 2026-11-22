import { useRef, useState } from 'react';
import { Alert, View } from 'react-native';
import { removeGuest, saveGuest, type GuestAccount, type GuestMachine, type Server } from '@hangar/core';
import { CheckRow } from '../config/CheckRow';
import { SectionCard } from '../config/SectionCard';
import { MachineField, MachineGroup, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { failureText, thrownText } from './guestTools';
import * as m from '../../paraglide/messages';

/** Adicionar ou editar um convidado; a conta fica em `store`, e cada máquina marcada recebe o acesso dele. */
export function GuestSheet({ store, machines, guest, onClose, onChanged }: {
  store: Server; machines: GuestMachine[]; guest: GuestAccount | null; onClose: () => void; onChanged: () => void;
}) {
  const editing = !!guest;
  const [user, setUser] = useState(guest?.user ?? '');
  const [password, setPassword] = useState('');
  const [seesOwner, setSeesOwner] = useState(guest?.seesOwner ?? false);
  const [ownerSees, setOwnerSees] = useState(guest?.ownerSees ?? true);
  // Servidor marcado = tem pasta; a ordem é a de marcar.
  const [roots, setRoots] = useState<Record<string, string>>(() => Object.fromEntries((guest?.servers ?? []).map((s) => [s.serverId, s.root])));
  const [errors, setErrors] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  // O que as máquinas já têm: depois de uma gravação que falhou no meio, salvar de novo parte daqui e não cria o acesso duas vezes.
  const prev = useRef<GuestAccount | null>(guest);
  const known = new Set(machines.map((machine) => machine.server.id));
  // Máquina que saiu da lista deste aparelho continua na conta, sem poder ser trocada daqui.
  const gone = (guest?.servers ?? []).filter((s) => !known.has(s.serverId));
  const labelOf = (id: string) => machines.find((machine) => machine.server.id === id)?.server.label
    ?? guest?.servers.find((s) => s.serverId === id)?.label ?? id;

  const toggle = (id: string) => setRoots((before) => {
    const next = { ...before };
    if (id in next) delete next[id];
    else next[id] = '';
    return next;
  });

  const save = async () => {
    if (busy) return;
    const marked = Object.entries(roots);
    const problems = [
      !user.trim() ? m.convidados_usuario_obrigatorio() : '',
      !editing && !password ? m.guests_password_required() : '',
      !marked.length ? m.convidados_servidor_obrigatorio() : '',
      marked.some(([, root]) => !root.trim()) ? m.convidados_pasta_obrigatoria() : '',
    ].filter(Boolean);
    setErrors(problems);
    if (problems.length) { haptic.error(); return; }
    setBusy(true);
    const created = !prev.current;
    try {
      const draft = { user, ...(password ? { password } : {}), seesOwner, ownerSees,
        servers: marked.map(([serverId, root]) => ({ serverId, root: root.trim() })) };
      const result = await saveGuest(store, machines, draft, prev.current);
      const nameTaken = result.storeFailed && result.errors.some((e) => e.code === 'guest_store_failed' && e.status === 409);
      if (nameTaken && created) {
        // Nome recusado: tira os acessos que acabaram de nascer, para outro nome começar limpo.
        await removeGuest(store, machines, result.saved).catch(() => []);
      } else {
        prev.current = result.saved;
      }
      if (result.errors.length) {
        setErrors(result.errors.map((failure) => failureText(failure, labelOf)));
        haptic.error();
        if (!result.storeFailed) onChanged();
        return;
      }
      toast.ok(m.convidados_salvo());
      onChanged();
      onClose();
    } catch (error) {
      setErrors([thrownText(error)]);
      haptic.error();
    } finally {
      setBusy(false);
    }
  };

  const remove = () => {
    if (!guest) return;
    Alert.alert(m.convidados_remover(), m.convidados_remover_aviso({ usuario: guest.user }), [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.convidados_remover(), style: 'destructive', onPress: () => {
        setBusy(true);
        setErrors([]);
        void removeGuest(store, machines, prev.current ?? guest).then((failures) => {
          if (failures.length) {
            setErrors(failures.map((failure) => failureText(failure, labelOf)));
            haptic.error();
            onChanged();
            return;
          }
          toast.ok(m.guests_removed());
          onChanged();
          onClose();
        }, (error: unknown) => { setErrors([thrownText(error)]); haptic.error(); }).finally(() => setBusy(false));
      } },
    ]);
  };

  return <MachineSheet title={editing ? m.convidados_editar() : m.convidados_adicionar()} onClose={onClose}>
    <MachineGroup>
      {editing ? <MachineRow title={m.login_usuario()} value={user} />
        : <MachineField label={m.login_usuario()} value={user} onChangeText={setUser} editable={!busy}
          textContentType="username" autoComplete="off" />}
      <MachineField label={editing ? m.guests_password_keep() : m.login_senha()} value={password} onChangeText={setPassword}
        editable={!busy} secureTextEntry textContentType="newPassword" autoComplete="new-password" stacked={editing} />
    </MachineGroup>
    <SectionCard icon="Server" title={m.convidados_servidores()}>
      {machines.map(({ server }) => {
        const marked = server.id in roots;
        return <View key={server.id}>
          <CheckRow title={server.label} disabled={busy} checked={marked} onToggle={() => toggle(server.id)}
            description={marked ? (roots[server.id] ? m.guests_folder_value({ pasta: roots[server.id] }) : undefined) : m.guests_no_access()} />
          {marked ? <MachineField label={m.convidados_pasta({ servidor: server.label })} value={roots[server.id]} editable={!busy}
            onChangeText={(root) => setRoots((before) => ({ ...before, [server.id]: root }))} placeholder={m.guests_folder_hint()} stacked /> : null}
        </View>;
      })}
      {gone.map((s) => <MachineRow key={s.serverId} title={s.label} description={m.convidados_servidor_fora_da_lista()}
        value={m.guests_folder_value({ pasta: s.root })} />)}
    </SectionCard>
    <MachineGroup>
      <MachineRow title={m.convidados_ve_minhas()} checked={seesOwner} disabled={busy} onPress={() => setSeesOwner(!seesOwner)} />
      <MachineRow title={m.convidados_eu_vejo()} checked={ownerSees} disabled={busy} onPress={() => setOwnerSees(!ownerSees)} />
    </MachineGroup>
    {errors.map((text) => <Notice key={text} text={text} error />)}
    <MachineGroup>
      <MachineRow title={busy ? m.convidados_salvando() : m.convidados_salvar()} action disabled={busy} onPress={() => { void save(); }} />
      {editing ? <MachineRow title={m.convidados_remover()} danger action disabled={busy} onPress={remove} /> : null}
    </MachineGroup>
  </MachineSheet>;
}

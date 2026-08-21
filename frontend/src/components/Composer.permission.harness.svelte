<script lang="ts">
  import Composer from './Composer.svelte';
  import type { State } from '@hangar/core';

  let state = $state<State>('working');
  // exposto pro teste chamar
  export function setState(v: State) { state = v; }
  export function getState() { return state; }
</script>

<Composer
  sessionName="perm-test"
  sessionState={state}
  status={null}
  onSend={async () => {}}
  onCommand={() => {}}
  onInterrupt={() => {}}
  onOpenGit={() => {}}
  onOpenPreview={() => {}}
/>

<script lang="ts">
  import { onMount } from "svelte";
  import { fetchUsers } from "$lib/api/client";

  let users = $state([]);

  /** Reload from the API. */
  async function reload() {
    users = await fetchUsers([1]);
  }

  const onClick = () => {
    reload();
  };

  onMount(reload);
</script>

<button onclick={onClick}>Reload</button>
<button on:click|preventDefault={() => reload()}>Force</button>
{#each users as u}<p>{u.name}</p>{/each}

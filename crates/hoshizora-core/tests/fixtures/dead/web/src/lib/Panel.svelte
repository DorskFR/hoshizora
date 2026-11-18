<script lang="ts">
	import { fmt } from './util';

	const fallbackKey = (o: number) => String(o);
	let { getKey = fallbackKey } = $props();
	let items = $state([3, 1, 2]);
	const rows = $derived.by(() => sortRows(items));

	function sortRows(xs: number[]) {
		return [...xs].sort();
	}

	function label(n: number) {
		return `#${n}`;
	}

	function attach(node: HTMLElement) {
		node.focus();
	}

	function persist() {
		localStorage.setItem('k', '1');
	}

	function commit() {
		persist();
	}

	function hrefOf(n: number) {
		return `/item/${n}`;
	}

	function orphan() {
		return 0;
	}
</script>

{#each rows as row}
	{@const text = fmt(row)}
	<a href={hrefOf(row)} {@attach attach}>{label(row)} {text}</a>
{/each}
<button onclick={commit}>save</button>

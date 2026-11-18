<script lang="ts">
	import { queries } from './queries';

	const history = queries.forItem(() => 1);
	const initial = untrack(() => clipRange());

	function clipRange() {
		return { start: 0 };
	}

	const REASONS: Record<string, () => string> = {
		stalled: () => 'stalled',
		other: () => ''
	};

	function reason(key: string) {
		return REASONS[key]();
	}
</script>

<p>{$history.data} {initial.start} {reason('other')}</p>

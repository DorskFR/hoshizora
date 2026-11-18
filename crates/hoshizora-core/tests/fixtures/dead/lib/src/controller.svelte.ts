let mounted = $state<number[]>([]);

export const hmm = {
	open() {
		return mounted.at(-1);
	},
	get mounted() {
		return mounted.length > 0;
	}
};

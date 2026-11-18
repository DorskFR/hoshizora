import type { Actions } from './$types';

export const load = async () => loadRows();

export const actions = {
	save: async () => saveRow()
} satisfies Actions;

function loadRows() {
	return [];
}

function saveRow() {
	return 1;
}

function unusedRoute() {
	return 2;
}

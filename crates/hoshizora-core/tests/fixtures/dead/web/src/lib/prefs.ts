function defaultStorage() {
	return localStorage;
}

function defaultSleep(ms: number) {
	return ms;
}

export class Prefs {
	constructor(private storage = defaultStorage()) {}

	get items() {
		return [this.storage];
	}

	get count() {
		return 1;
	}
}

export function poll({ sleep = defaultSleep } = {}) {
	return sleep(1);
}

export function read(prefs?: Prefs) {
	return [prefs?.count, prefs?.items.map((x) => x)];
}

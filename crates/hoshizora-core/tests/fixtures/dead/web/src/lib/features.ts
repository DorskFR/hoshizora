const finiteWith = (mood: string) => (t: string) => t === mood;

const FEATURES: Record<string, (t: string) => boolean> = {
	present: finiteWith('Pres'),
	plural: (t) => t.endsWith('s')
};

export function matches(key: string, t: string) {
	return FEATURES[key](t);
}

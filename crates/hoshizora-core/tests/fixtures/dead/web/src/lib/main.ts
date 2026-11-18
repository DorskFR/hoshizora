import { Estimator, Tool, toggle } from './scene';
toggle(new Tool());

function tick() {
	return 1;
}

function activate() {
	return 2;
}

function exposed() {
	return 3;
}

function mapped(n: number) {
	return n * 2;
}

function lonely() {
	return 4;
}

const m = await import('./dyn');
m.runA();
(await import('./dyn')).runB();
import('./dyn').then((mod) => mod.runC());
setTimeout(tick, 10);
export const handlers = { onactive: activate };
Object.assign(window, { exposed });
const doubled = [1, 2].map(mapped);
const est = new Estimator();
console.log(doubled, est.lastAt);
import { Prefs, poll, read } from './prefs';
console.log(read(new Prefs()), poll());
import { matches } from './features';
console.log(matches('present', 'Pres'));

import { Base } from 'engine';

export class Scene extends Base {
	private tex = makeTexture(2);

	constructor() {
		super();
		this.bind();
	}

	private bind() {
		return this.tex;
	}
}

function makeTexture(n: number) {
	return n;
}

export class Estimator {
	get lastAt() {
		return 1;
	}

	get unreadGetter() {
		return 2;
	}
}

export class Tool {
	set mode(on: boolean) {
		console.log(on);
	}
}

export function toggle(tool: Tool) {
	tool.mode = true;
}

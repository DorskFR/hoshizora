import { walkMarkup } from './ctx.mjs';

export default /** @type {any} */ ({
	id: 'rule',
	check(ctx) {
		walkMarkup(ctx, () => {});
	}
});

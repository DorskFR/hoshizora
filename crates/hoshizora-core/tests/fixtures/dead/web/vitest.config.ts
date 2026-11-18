export default {
	resolve: {
		alias: { '$app/paths': new URL('./src/lib/testing/appPaths.ts', import.meta.url).pathname }
	}
};

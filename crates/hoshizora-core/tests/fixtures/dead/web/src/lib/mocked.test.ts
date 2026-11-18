import { vi } from 'vitest';

vi.mock('./mocked.svelte', () => ({ mockedButUnused: () => 2 }));

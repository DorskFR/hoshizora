import { table } from 'orm';

export const users = table('users', () => ({ id: 1 }));

export const posts = table('posts', () => ({ id: 2 }));

import { drizzle } from 'orm';
import * as schema from './schema';

export const db = drizzle({ schema });

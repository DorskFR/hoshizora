import { Pool } from "pg";

const pool = new Pool();

export class Catalog {
  async fetchIds(sql: string): Promise<string[]> {
    const res = await pool.query(sql);
    return res.rows;
  }

  async albums(ids: string[]): Promise<string[]> {
    const res = await pool.query("SELECT name FROM albums WHERE id = ANY($1)", [ids]);
    return res.rows;
  }
}

export class Weather {
  async fetchWeather(city: string): Promise<string> {
    const res = await fetch(`https://weather.example/${city}`);
    return res.text();
  }
}

export async function dependentViaBinding(catalog: Catalog, weather: Weather, sql: string) {
  const ids = await catalog.fetchIds(sql);
  const report = await weather.fetchWeather(ids[0]);
  return report;
}

export async function dependentViaDestructuring(catalog: Catalog, weather: Weather, sql: string) {
  const [first] = await catalog.fetchIds(sql);
  const report = await weather.fetchWeather(first);
  return report;
}

export async function dependentViaIntermediate(catalog: Catalog, weather: Weather, sql: string) {
  const ids = await catalog.fetchIds(sql);
  const city = ids.map((i) => i.trim()).join(",");
  const report = await weather.fetchWeather(city);
  return report;
}

export async function commitBarrier(catalog: Catalog, sql: string) {
  await pool.query("COMMIT");
  const ids = await catalog.fetchIds(sql);
  return ids;
}

export async function independent(catalog: Catalog, weather: Weather, sql: string, city: string) {
  const ids = await catalog.fetchIds(sql);
  const report = await weather.fetchWeather(city);
  return { ids, report };
}

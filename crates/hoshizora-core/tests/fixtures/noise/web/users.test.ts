export async function fetchAll(ids: string[]) {
  for (const id of ids) {
    await fetch(`https://api.example.com/users/${id}`);
  }
}

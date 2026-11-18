export async function mode(id: string) {
  return fetch("https://api.example.com/modes/" + id);
}

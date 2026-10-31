import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const bump = () => setN(n + 1);
  function reset() {
    setN(0);
  }
  return (
    <div>
      <button onClick={bump}>+</button>
      <button onClick={() => reset()}>0</button>
      <input onChange={(e) => setN(Number(e.target.value))} />
    </div>
  );
}

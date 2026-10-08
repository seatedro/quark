import { useState } from "react";
import { CommandList } from "./commands";
import { TripList } from "./trips";

export function App() {
  const [query, setQuery] = useState("");
  const [showHelp, setShowHelp] = useState(false);

  return (
    <main className="atlas">
      <header className="toolbar">
        <input
          aria-label="Search places"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <button onClick={() => setShowHelp(true)}>Help</button>
      </header>
      <TripList query={query} />
      {showHelp && <CommandList onClose={() => setShowHelp(false)} />}
    </main>
  );
}

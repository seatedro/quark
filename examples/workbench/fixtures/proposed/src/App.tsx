import { useEffect, useState } from "react";
import { CommandList, commandForKey } from "./commands";
import { TripList } from "./trips";

export function App() {
  const [query, setQuery] = useState("");
  const [showHelp, setShowHelp] = useState(false);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const command = commandForKey(event);
      if (!command) return;
      event.preventDefault();
      if (command.id === "view.help") setShowHelp(true);
      else command.run();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

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

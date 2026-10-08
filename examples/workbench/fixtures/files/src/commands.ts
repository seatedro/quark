export interface Command {
  id: string;
  title: string;
  run: () => void;
}

export const commands: Command[] = [
  { id: "trip.new", title: "New trip", run: () => {} },
  { id: "place.pin", title: "Pin place", run: () => {} },
  { id: "view.help", title: "Show help", run: () => {} },
];

export function CommandList(props: { onClose: () => void }) {
  return null;
}

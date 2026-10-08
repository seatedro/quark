export interface Trip {
  id: number;
  name: string;
  places: string[];
}

export function TripList(props: { query: string }) {
  return null;
}

export function matches(trip: Trip, query: string): boolean {
  const q = query.trim().toLowerCase();
  return q === "" || trip.name.toLowerCase().includes(q);
}

// The user's order for a list (the sidebar's agents): sorting by it, and the arithmetic of
// dragging one item to a new slot. Pure, so it is tested without a DOM.

/** `items` in the saved order; ids it doesn't list keep their own order, after the listed ones. */
export function sortByOrder<T extends { id: string }>(items: T[], order: string[]): T[] {
  const rank = new Map(order.map((id, i) => [id, i]));
  const key = (a: T, i: number) => rank.get(a.id) ?? order.length + i;
  return items
    .map((a, i) => ({ a, k: key(a, i) }))
    .sort((x, y) => x.k - y.k)
    .map((x) => x.a);
}

/** The order to save: what is on screen now, then remembered ids that aren't (hidden agents). */
export function mergeOrder(visible: string[], saved: string[]): string[] {
  const seen = new Set(visible);
  return [...visible, ...saved.filter((id) => !seen.has(id))];
}

/** `list` with the item at `from` moved to `to`. */
export function moveItem<T>(list: T[], from: number, to: number): T[] {
  const out = [...list];
  const [it] = out.splice(from, 1);
  out.splice(to, 0, it);
  return out;
}

/**
 * The slot a dragged item lands in: past the midpoint of a neighbour, it takes that
 * neighbour's place. `mids` are the items' midpoints at rest, `center` the dragged item's.
 */
export function dropIndex(mids: number[], from: number, center: number): number {
  for (let i = 0; i < from; i++) if (center < mids[i]) return i;
  for (let i = mids.length - 1; i > from; i--) if (center > mids[i]) return i;
  return from;
}

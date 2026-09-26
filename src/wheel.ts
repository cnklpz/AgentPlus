// Telling trackpad momentum from the user's own scrolling, for the overscroll bounce in
// motion.ts. After the fingers lift, a trackpad keeps sending wheel events, each a little
// smaller than the last, for a second or more.

/** What the previous wheel events looked like. */
export interface WheelTrack {
  /** Time and size of the last event. */
  at: number; last: number;
  /** How many events in a row have shrunk; whether the stream is momentum. */
  decaying: number; coasting: boolean;
}

/** Whether this wheel event is trackpad momentum rather than the user pushing. */
export function isMomentum(state: WheelTrack, time: number, size: number): boolean {
  const burst = time - state.at < 80;
  state.decaying = burst && size < state.last ? state.decaying + 1 : burst && size === state.last ? state.decaying : 0;
  state.at = time;
  state.last = size;
  // A pause or a harder push is the fingers again.
  if (!burst || state.decaying === 0) state.coasting = false;
  if (state.decaying >= 3) state.coasting = true;
  return state.coasting;
}

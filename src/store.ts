// A module-level value that components can subscribe to (language, privacy mode, updater).
import { useSyncExternalStore } from "react";

export interface Store<T> {
  /** Call after the value returned by `get` changed: subscribed components re-render. */
  notify(): void;
  /** The current value; re-renders the component when `notify` is called. */
  use(): T;
}

export function createStore<T>(get: () => T): Store<T> {
  const subs = new Set<() => void>();
  const subscribe = (f: () => void) => {
    subs.add(f);
    return () => { subs.delete(f); };
  };
  return {
    notify: () => subs.forEach((f) => f()),
    use: () => useSyncExternalStore(subscribe, get),
  };
}

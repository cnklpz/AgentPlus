/** Running inside the Tauri app (not the plain-browser preview of `npm run dev`). */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

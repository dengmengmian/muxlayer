import "@testing-library/jest-dom";

// Node 25+ ships a built-in `localStorage`/`sessionStorage` stub. Vitest's
// jsdom environment only copies window keys that are not already globals, so
// the Node stub (which lacks getItem/clear) shadows jsdom's Storage. CI runs
// Node 22 where this does not happen; mirror that here.
//
// Plain JS syntax on purpose: ESLint parses this root-level file without the
// TypeScript parser (see eslint.config.js), so `as` casts break `pnpm lint`.
const jsdomWindow = Reflect.get(globalThis, "jsdom")?.window;
if (jsdomWindow) {
  for (const key of ["localStorage", "sessionStorage"]) {
    const current = Reflect.get(globalThis, key);
    if (typeof current?.clear !== "function") {
      Object.defineProperty(globalThis, key, {
        value: jsdomWindow[key],
        configurable: true,
        writable: true,
      });
    }
  }
}

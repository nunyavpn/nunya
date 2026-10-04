# Design

A style guide that renders itself from the app.

```bash
npm run design      # opens http://localhost:1420/design/
```

## What it is

`styleguide.ts` does not contain a palette, an icon list or a component catalogue. It reads all
three out of the running app:

| Shown | Read from |
| --- | --- |
| Every colour, shadow and font token | `src/styles.css`, via the browser's own CSSOM |
| Every icon | `ICON_NAMES` in `src/views/icons.ts` |
| The status card, in four states | `StatusCard` itself, mounted with a plain model |
| The tray icon, in four states and both appearances | `trayIcon` and `shieldState`, fed each theme's tokens |
| Rows, group headers, quota bars, controls | the real classes and the real `place` / `latency` / `bars` helpers |

So there is no second copy of the design to keep in step. Add a token to `styles.css` and it shows
up here — ungrouped, under **Other**, which is the page telling you to file it. Add an icon and it
appears in the grid. Change `--brand` and every specimen moves with it.

## Both themes at once

The app follows the system setting and has no theme switch, so a guide that rendered only the
current theme would show half the palette with no way to see the rest. Each colour chip is split:
light value on the left, dark on the right, both read out of the `prefers-color-scheme` block
rather than guessed. The page chrome itself still follows your system setting, so you can check
either one in situ.

## What is restated, and why

`StatusCard` takes a plain `StatusModel` and touches no store, so the page mounts the real
component. Every other view subscribes to the store in its constructor and cannot be mounted
against a fixture, so `locRow` and friends mirror the markup in `src/views/locations.ts`. That is
the one place this page can drift from the app: the classes and helpers are real, but the element
nesting is a copy. If a row gains an element, it has to gain one here too.

The alternative — making the views take their data as an argument instead of reading the store —
would remove the duplication, and is worth doing if the gallery grows. It is not worth doing for
one function.

## It never ships

`vite build` takes only the root `index.html` as its entry, so nothing in this directory is bundled.
The page is a development tool served by the dev server, and the sample servers in it use hosts
under `example.net`, which RFC 2606 reserves so they can never be registered.

## Not here yet

- **Manrope.** `--ui` names it first, but the app's CSP is `default-src 'self'` and no remote font
  host is allowed, so the face has to ship in the bundle. Until it does, the system stack stands in
  and the guide renders what you will actually see.
- **A type scale.** Sizes are set per component, between 9.5px and 26px. The guide shows the two
  families and says so rather than inventing a scale the app does not use.
- **The map, rail, panels and sheets.** Only the components above are covered so far.

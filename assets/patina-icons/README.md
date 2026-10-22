# PatinaTorrent icon explorations

Three original application icon candidates and a matching family of 30 compact state icons. Open **review.html** for a local interactive gallery with light/dark previews, actual-size proofs, state filters and download links. The gallery works directly from disk; all image paths are relative.

## Application candidates

| Candidate | Direction | Master | Windows icon |
|---|---|---|---|
| **01 · Verdigris** (recommended) | Ivory P ending in a download arrow; copper-rimmed teal medallion | [PNG](candidates/01-verdigris.png) | [ICO](exports/app/01-verdigris.ico) |
| **02 · Exchange** | Copper and ivory arrows express peer-to-peer transfer | [PNG](candidates/02-exchange.png) | [ICO](exports/app/02-exchange.ico) |
| **03 · Foundry** | Copper gear, teal enamel and a bold download symbol | [PNG](candidates/03-foundry.png) | [ICO](exports/app/03-foundry.ico) |

Verdigris links the name to a distinctive P/download symbol. Exchange emphasizes sharing, and Foundry emphasizes the industrial Rust character. These are reviewable candidates; choosing a direction in the gallery changes only that preview.

Each master is a 1254 × 1254 PNG with a real alpha channel. PNG exports: **16, 20, 24, 32, 48, 64, 128, 256, 512 px**. Every Windows ICO contains **16, 20, 24, 32, 48, 64, 128, 256 px** PNG frames. The ICOs target the project's Windows 10+ baseline. These raster candidates use restrained copper/enamel shading; their small-size exports are resampled proofs, not separately hand-hinted pixel artwork.

## State family

[states/README.md](states/README.md) explains the exact semantics and precedence. [states/manifest.json](states/manifest.json) maps the assets to current Rust states and frontend conditions.

- 18 current torrent variants, including force-start, queue/checking, completed/incomplete pause, stopped and finished.
- Four tracker states and two network states.
- One supplemental wanted-files-complete core detail, four future states and one reusable force-start overlay.

Editable SVGs use a **16 × 16** view box. Transparent PNG exports are in `exports/states/{16,20,24,32,48,64}/`. The classic light-interface palette is teal for downloads, blue for seeding, amber for checks/queues, gray for inactivity, and red for errors. Shapes carry the meaning independently of color.

## Files

- [review.html](review.html): interactive gallery.
- [app-candidates.png](app-candidates.png): application comparison sheet, including actual-size light/dark proofs.
- [state-icons.png](state-icons.png): complete state comparison sheet.
- [gallery-preview.png](gallery-preview.png): desktop gallery screenshot.
- `candidates/`: untouched generated transparent PNG masters.
- `states/`: editable original SVGs, documentation and state mapping.
- `exports/`: ready-to-use PNG and ICO assets.
- [manifest.json](manifest.json): candidate names and export sizes.
- [prompts.json](prompts.json): exact prompts used with the built-in image_gen tool.
- [validation.json](validation.json): export validation summary.

01 Verdigris and the state family are connected to the application, provisionally until a direction is chosen: [INTEGRATION.md](INTEGRATION.md) lists where each file is used and how to switch candidates. The assets are original; no artwork was taken from other torrent clients. App candidates were created using the built-in image_gen tool; state icons were drawn directly as vectors. Repository licensing applies.

To regenerate exports and comparison sheets with Node.js and the `sharp` library:

```text
node build.cjs
```

If sharp is outside the normal module path, pass `--sharp /absolute/path/to/sharp`. This helper is for design exports only and adds no runtime dependency to PatinaTorrent. `review.template.html` is the editable gallery source; the build embeds the manifests into `review.html`.

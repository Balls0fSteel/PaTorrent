# Postal Rook: small-size icon exploration

Eleven new image-generated concepts designed around recognition at 16 pixels. The user's preferred Postal Rook supplies the navy, brick-red and cream identity; the full walking bird and detailed perforated stamp were removed or simplified.

[Download all eleven ICOs](postal-rook-small-icons.zip) · [Full size comparison](comparison.png) · [Shortlist comparison](shortlist.png) · [Exact generation prompts](prompts.json)

## Recommended directions

- **09 Compact Courier:** strongest straightforward continuation of the cap-wearing bird. Closed projecting beak, clear eye, short neck.
- **10 Night Courier:** cream face on navy makes the face and eye particularly distinct. Strong alternative for an app tile.
- **11 Closed-beak Courier:** bird plus envelope, with fewer internal edges than 07.
- **01 Courier Head:** also a strong postal bird; compare with 09.
- **05 Postal Tower:** most geometric and robust, but changes the primary symbol from bird to chess piece.

These are visual judgments from the actual 16 px exports, not user recognition-test results. 02/08 gained unwanted crown-like shapes that can read as a chicken comb; 04 tends toward a duck. 06 retains the R but loses much of the postal/bird reading.

## Files and verification

- `sources/`: eleven unmodified generated masters.
- `icons/*.ico`: eleven Windows icon files, each containing 16, 20, 24, 32, 40, 48, 64, 96, 128 and 256 px frames.
- `icons/<candidate>/<size>.png`: individual PNG frames.
- `comparison.png`: original stamp plus all eleven candidates, actual-size exports on light/dark surround, nearest-neighbor 8x view of the 16 px frame, and 128 px reference.
- `shortlist.png`: compact comparison of the original and four recommended alternatives.
- `export_icons.py`: reproducible Lanczos size export and proof construction.
- `prompts.json`: verbatim prompts and generation settings. Built-in image generation, no reference images, opaque backgrounds.

All 110 embedded ICO frames were reopened and decoded successfully. Real 16/20/24/32/48 px proofs were visually reviewed. The artwork has opaque cream/navy backgrounds; it is not a monochrome adaptive tray glyph. The tiny frames are resampled generated pictograms, not manually pixel-hinted drawings. No production app resources were changed.

Open the comparison at 100% to judge native pixel sizes; chat or image viewers may scale the whole sheet. The enlarged 16 px column is diagnostic, not a claim of native-size readability.


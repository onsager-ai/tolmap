The two map documents `scripts/check-view-stability.mjs` pins by SHA-256, gzipped. `public/maps/` is generated and gitignored, so without these copies CI had no way to supply them. The viewer-check workflow (`.github/workflows/viewer-check.yml`) unpacks them over the `collect-maps.mjs` output before it runs the check.

When the check's pinned hashes move (a map rebuilt from a newer pipeline), replace the file here in the same change.

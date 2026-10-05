# Vendored front-end libraries

These files ship inside the FreEco.ai binary and run in the dashboard.
`cargo audit` does not see them, so this file records what they are, and
`SHA256SUMS` lets CI prove the shipped bytes are the ones recorded here.

| File | Library | Version | Source of the version |
|---|---|---|---|
| `alpine.min.js` | Alpine.js | 3.15.8 | `version:"3.15.8"` inside the file |
| `chart.umd.min.js` | Chart.js | 4.4.7 | file header |
| `highlight.min.js` | highlight.js | 11.11.1 | file header |
| `github-dark.min.css` | highlight.js theme "GitHub Dark" | not stated in the file | ships with highlight.js |
| `marked.min.js` | marked | 15.0.12 | file header |

Recorded 2026-10-01. To update a library: replace the file, update its row
here, and regenerate the checksums from this directory:

```sh
sha256sum alpine.min.js chart.umd.min.js highlight.min.js marked.min.js github-dark.min.css > SHA256SUMS
```

Check each new version against the library's security advisories before
replacing it: nothing does that automatically for these files.

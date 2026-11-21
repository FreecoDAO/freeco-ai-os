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
- [DOMPurify 3.2.7](https://github.com/cure53/DOMPurify) — Apache-2.0/MPL-2.0, sanitized HTML for markdown rendering. File: dompurify.min.js (sha256 40cff492b54353a55cdf9ffb369786539ffb7d54d25f97dd4a1e8975b309c35d).
- [KaTeX 0.16.21](https://github.com/KaTeX/KaTeX) — MIT, LaTeX math rendering. Files: katex.min.js, auto-render.min.js (sha256s in SHA256SUMS). Now bundled locally instead of loading from the CDN at runtime.

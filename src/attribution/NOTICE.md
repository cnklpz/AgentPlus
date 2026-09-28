# Model attribution: third-party notice

The attribution test scores answers with a port of [ModelTrace](https://github.com/xqy2006/ModelTrace)
by xqy2006, used under the MIT License (full text in `LICENSE-ModelTrace.txt`, also shown in the
attribution dialog under "Source and license").

Pinned commit: `df3a0f9d3e054c0dc02d6d586686db8daf8fa7c8`

| File here | Upstream file | Upstream blob SHA-256 (LF) |
|---|---|---|
| `unified_bank.json` (verbatim copy) | `data/unified_bank.json` | `1c2cb74d372f9f0f30d0dabbb7b7a838660d2f769a88d0c8489e4c662e088c21` |
| `LICENSE-ModelTrace.txt` (verbatim copy) | `LICENSE` | `238afb57e498990742f84b9ce6167b4e256ac06104a9938ce953bc41c14bea51` |
| `core.ts` (TypeScript port) | `static/fingerprint-core.js` | `83fa5bd611e18f8339122582335123c8ea168ed242298bb31f4e363abeeb6e4a` |
| `challenge.ts` (TypeScript port) | `static/challenge-browser.js` | `4a3868c9fe237cabde92f0d2bffaf32118f558f7a43fcc336874fecd2bf6be39` |

`bank.test.ts` checks the bundled bank against the hash above; the app never downloads a bank and
never contacts ModelTrace.

The ports keep the reference semantics: parser, minimum answer length
(`max(80, ceil(0.55 × requested))`), Hellinger and ordered-block features with the nuisance
projections and bank weights, per-answer scores averaged before one global softmax, and the
calibration temperature for the number of valid answers. Differences are only in form: camelCase
result fields, errors with stable codes instead of Chinese messages, `cv_accuracy` and the method
label left out of the result, and an injectable random source for the challenge generator.

`fixtures/reference.json` holds fixed answers with the results the original JavaScript computed
for them. Regenerate it from a ModelTrace checkout at the pinned commit with:

```sh
node scripts/attribution-fixtures.mjs <path to ModelTrace>
```

To move to a newer bank: copy the new `data/unified_bank.json`, update the commit and hash here
and in `bank.ts` (`BANK_SOURCE`), re-check `core.ts` / `challenge.ts` against the new upstream
sources, regenerate the fixture, and review `ALIASES` in `bank.ts`.

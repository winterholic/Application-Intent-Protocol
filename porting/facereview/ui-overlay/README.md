# FaceReview UI overlay (WBS 7.2.10)

The player of FaceReview is the user's repository `facereview-front-joowon`. This directory holds only the files that change when its
data access moves from the original REST API to AIP; it is meant to be copied over a checkout of the pinned revision
`2e7c021fa5abebd72278d841b8066a33d45cfbb3`. Decision: `docs/DECISIONS.md` D-P-UI-ADAPTER. Evidence: `product/VERIFICATION.md`.

**Status: slice 1 of the read side. The UI has not been built or run with this overlay, and no browser was used.**

| File | What it is | Tested |
|---|---|---|
| `src/api/readAdapter.ts` | The nine read functions of `src/api/youtube.ts` over an SDK client (`extension`, `writeExtension`, `apply`). Same names, arguments and results as the original; parses the JSON text the extensions return | `tests/readAdapter.test.ts` (fake client, the original's golden answer for the watch detail), run by `tools/verify.sh` |
| `src/api/youtube.ts` | The overlay file itself: re-exports the adapter, and `toggleBookmark` fails with `PORT.NOT_PORTED` | no |
| `src/api/aipClient.ts` | The one `@aip/sdk` client of the player, with the member's current access token | no (needs the UI tree, the generated binding and the SDK tarball) |

Generate the binding the client imports (`src/aip/facereview.ts`) from the definition, do not write it by hand:

```sh
aip service gen --lang ts --out src/aip/facereview.ts product/tests/fixtures/facereview-mysql/facereview.aip
```

## Not ported yet

- Guests: an extension call needs a signed session, so the home screen of a visitor is not served (open problem in D-P-UI-ADAPTER).
- `toggleBookmark`, likes, comments, the video request form, login/signup/reissue/logout, profile and password screens.
- The Socket.IO player (`src/socket`): waits for the model decision of WBS 7.2.6.
- Build of the overlaid UI, `VITE_AIP_URL`/dev proxy, and the browser run that closes WBS 7.2.10.

# FaceReview UI overlay (WBS 7.2.10)

The player of FaceReview is the user's repository `facereview-front-joowon`. This directory holds only the files that change when its
data access moves from the original REST API to AIP; it is meant to be copied over a checkout of the pinned revision
`2e7c021fa5abebd72278d841b8066a33d45cfbb3`. Decision: `docs/DECISIONS.md` D-P-UI-ADAPTER. Evidence: `product/VERIFICATION.md`.

**Status: read, auth, and write data adapters are present.** The generated TypeScript SDK and these adapters completed a real member API journey through the AIP service and dedicated Linux CI fixtures at code SHA `1033269ec1389ddd7b750b6ad9f5e53bd100a53b` (Verify run [39851918809](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39851918809)). A React browser journey is in progress: the previous run rejected its generated binding with `WIRE.CONTRACT_MISMATCH`; CI now supplies the same combined definition to UI generation and the real service test, with a pre-browser fingerprint check. Do not count the browser journey complete until its Linux Verify and MySQL bookmark assertions pass.

| File | What it is | Tested |
|---|---|---|
| `src/api/readAdapter.ts` | Nine video read functions over the SDK client; parses the extension JSON into the existing UI shape | adapter unit tests plus real `getAllVideo` member journey against generated SDK and service |
| `src/api/authAdapter.ts`, `src/api/auth.ts` | Existing `/auth/*` flows and one-refresh handling for protected SDK requests | adapter unit tests; real login, one reissue, logout, and revoked-token check |
| `src/api/writeAdapter.ts` | Bookmark/like create/remove, comment create/edit/delete, and video request recommendation | adapter unit tests; real bookmark on/off and comment create/edit/delete; create uses SDK `bundle({atomic:[...]})` and transitions use `apply()` |
| `src/api/youtube.ts`, `src/api/aipClient.ts` | Overlay exports and the player's configured SDK client | included in the pinned UI TypeScript/Vite build; server journey imports the read/write adapters directly |

The exact CI command and route coverage are documented in [product verification](../../../product/VERIFICATION.md). The journey uses only the dedicated member/video fixture and does not run model inference.

Generate the binding the client imports (`src/aip/facereview.ts`) from the definition, do not write it by hand:

```sh
aip service gen --lang ts --out src/aip/facereview.ts --sdk-import @aip/sdk <the exact definition used by the AIP service>
```

`porting/facereview/apply-ui-overlay.sh <clean-pinned-ui> [definition.aip]` accepts that definition as its second argument. The Linux browser workflow passes one combined file containing the relational and authentication operation fixtures to both the overlay generator and the service test.

## Remaining UI journey scope

- Visitor landing behavior remains an open policy decision in D-P-UI-ADAPTER.
- The full React login flow and signed-in screens have not been run against the AIP service in a browser. The server journey covers adapter/API calls, not mounted-page interactions.
- The rest of the 25 UI API routes, including profile/password pages and API field-by-field screen parity, remain outside the minimum member journey.
- The Socket.IO player (`src/socket`): waits for the model decision of WBS 7.2.6.
- Full static prerender, production `VITE_AIP_URL`/dev proxy setup, and the signed-in browser run that closes WBS 7.2.10.

# FaceReview UI overlay (WBS 7.2.10)

The player of FaceReview is the user's repository `facereview-front-joowon`. This directory holds only the files that change when its
data access moves from the original REST API to AIP; it is meant to be copied over a checkout of the pinned revision
`2e7c021fa5abebd72278d841b8066a33d45cfbb3`. Decision: `docs/DECISIONS.md` D-P-UI-ADAPTER. Evidence: `product/VERIFICATION.md`.

**Status: read, auth, and write data adapters are present.** The generated TypeScript SDK and these adapters completed a real member API journey through the AIP service and dedicated Linux CI fixtures at code SHA `1033269ec1389ddd7b750b6ad9f5e53bd100a53b` (Verify run [39851918809](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39851918809)). A pinned React login → fixture video → bookmark on/reload and off/reload browser journey passed with its MySQL assertions at SHA `b504f62161fb9e3065ac9da9972ebe9928e0c315` (Verify run [39885042974](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39885042974)); the earlier `WIRE.CONTRACT_MISMATCH` did not recur. A later diagnostic run verified the real `Comment.read` requests after correcting the query (`WIRE.UNKNOWN_KEY` cleared), but its full Verify failed at the bookmark accessibility assertion while an external YouTube media error modal covered the control (run [39896024733](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39896024733)). Keep that failure-state evidence distinct from the successful earlier bookmark journey. See [product verification](../../../product/VERIFICATION.md) for scope and evidence.

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
- Only the member home/list/watch bookmark slice has been run in the pinned React browser against AIP. The server journey covers additional adapter/API calls, not mounted-page interactions for every signed-in screen.
- The `/my` → `/edit` profile write is now connected through `aipWrites.updateProfile` and one atomic generated-SDK bundle at code SHA `d29f1d372505e26fa8bec60062f0e6c036a771a3`. Verify `39900453676` is the first CI run containing the form write and MySQL persistence assertions; it was in progress at 2026-10-23 04:30:40 UTC (core step 10 running; browser steps 11–13 pending). The older profile-read run `39899325826` (`dd59658274e93c65bd32c9f30ff0506c35aab982`) was cancelled before browser verification and is not evidence for the new write.
- The full 25 unique user API paths and separate administrator API/screen inventory, with API/SDK versus browser evidence, are in [product verification](../../../product/VERIFICATION.md#2026-10-23-wbs-7210-profile-write-overlay-and-route-coverage-code-sha-d29f1d372505e26fa8bec60062f0e6c036a771a3-verify-pending). The password change page's email challenge, code exchange, and reset routes have source overlay adapters at SHA `30dd4e9eb016319d162d4f27bd8e2ca38164d351`; confirmed withdrawal uses generated SDK `User.withdraw` at SHA `6b1a8d4f70688caa9a61ed32bba6095c86fae7f9`. Browser verification for these new bindings is pending Verify `39901115868`.
- The Socket.IO player (`src/socket`): waits for the model decision of WBS 7.2.6.
- Full static prerender, production `VITE_AIP_URL`/dev proxy setup, and the signed-in browser run that closes WBS 7.2.10.

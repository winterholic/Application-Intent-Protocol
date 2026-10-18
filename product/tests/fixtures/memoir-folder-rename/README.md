# Memoir folder rename slice

This is one PostgreSQL-backed application exercise selected from the existing `port-trials/memoir-notes` candidate. It covers one folder update and does not count as a complete Memoir port, a FaceReview gate, or authentication/authorization WBS work.

## Fixed source and comparison

- Local source repository: `/Users/winterholic/development/projects/memoir/memoir-backend`.
- Reviewed commit: `26396070917ee065b988b0de296baf54f6266322`; local `origin/main` ref matched this SHA.
- The source worktree had 36 modified or untracked paths. Its contents were not changed. The source files used for comparison were read from the committed tree with `git show HEAD:<path>`; no pull was attempted to avoid overwriting or disturbing the user's uncommitted changes. The remote was not fetched, so current upstream freshness is unverified.
- Source license was not assessed as this is the user's own project; no source implementation was copied.
- The source service and source database were not run or contacted.

At the reviewed commit:

| Source | Observed rule/model | AIP slice |
|---|---|---|
| `FolderService.updateFolder` and `FolderRepository.findByIdAndMemberId` | Updates are scoped by both folder ID and the authenticated member ID. The service sets `name` and `sortOrder`. | `Folder.rename` patches those same two fields only when `owner = actor` and `deleted = false`. |
| `FolderService.findFolders`; `Folder` entity `@SQLRestriction("deleted_at IS NULL")` | Lists a member's folders; logically deleted folders are excluded. | AIP reads rows owned by the actor and filters `deleted = false`. |
| `Folder.java`; its inherited logical deletion model | UUID primary key, required Member FK, optional parent Folder, non-null name and sort order, soft deletion timestamp. | UUID and owner relation are retained. The synthetic comparison schema includes parent and `deleted_at`; the AIP projection tests name/sort order and a derived deleted flag. |
| `FolderController.updateFolder`; `FolderUpdateReq` | `PUT /api/v1/folders/{id}` obtains identity from `@CurrentMemberId`, requires the plan annotation, requires nonblank name, and accepts `sortOrder`. | The test uses a valid name and sort order. The controller/plan entitlement and exact whitespace validation boundary are not ported or tested. A denied AIP target returns `WIRE.MISSING_TARGET`; source HTTP error/response equivalence was not tested. |

The runtime request path is AIP service → generated TypeScript binding → installed `@aip/sdk`; it does not proxy the source controller or ORM. The test seeds only synthetic, source-shaped rows into a dedicated temporary PostgreSQL database and a separate AIP-owned schema. Owner read/update, an unrelated actor's empty read and denied update, and the stored AIP row after each request are compared. The synthetic source-shaped row stays unchanged. This is **code-based application / real-database AIP verification**, not original E2E or whole-product equivalence.

For the selected columns, the comparison is `folders.id UUID → Folder.id Uuid`, `folders.member_id → Folder.owner`, `folders.name VARCHAR(255) → Folder.name Text(1..255)`, `folders.sort_order INT → Folder.sortOrder Int`, and `folders.deleted_at IS NOT NULL → Folder.deleted`. The fixture also preserves the source `users` table, `parent_id`, and non-null timestamps. The parent relation and timestamps are not exposed by this AIP slice. AIP's `owner` name is a semantic mapping of the source Member FK.

Excluded: parent folder operations, plan entitlements, folder creation/deletion, nonblank/length edge-case equivalence, source API startup, and production data/settings. The existing `port-trials/memoir-notes` exercise covers Note create/favorite/delete; this separate folder rename operation was not in the existing `product/tests/` verification cases.

## Run

```sh
node --test product/tests/memoir-folder-rename.test.mjs
```

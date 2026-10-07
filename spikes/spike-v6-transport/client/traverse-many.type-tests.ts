import { connect } from "../../../product/sdk/index.ts";
import type { ExtensionBinding } from "../../../product/sdk/index.ts";

type Contract = {
  Post: {
    root: true;
    fields: { id: number; title: string };
    traverse: { author: { target: "Comment"; select: "body" } };
    traverseMany: { comments: { target: "Comment"; select: "id" | "body"; maxLimit: 3 } };
    filterFields: {};
    filter: never;
    sort: "id" | "title";
    maxRows: 10;
    maxOffset: 20;
    cursor: true;
  };
  Comment: {
    root: false;
    fields: { id: number; body: string };
    traverse: {};
    filterFields: {};
    filter: never;
    sort: never;
    maxRows: 0;
  };
  Audit: {
    root: true;
    fields: { id: number };
    traverse: {};
    filterFields: {};
    filter: never;
    sort: "id";
    maxRows: 5;
  };
};

const binding = {
  fingerprint: "a".repeat(64),
  readDescriptors: {
    Post: {
      root: true,
      maxRows: 10,
      fields: { id: { type: "Id", nullable: false }, title: { type: "Text", nullable: false } },
      traverse: { author: { target: "Comment", select: ["body"] } },
      traverseMany: { comments: { target: "Comment", select: ["id", "body"], maxLimit: 3 } },
    },
    Comment: { root: false, maxRows: 0, fields: { id: { type: "Id", nullable: false }, body: { type: "Text", nullable: false } }, traverse: {} },
    Audit: { root: true, maxRows: 5, fields: { id: { type: "Id", nullable: false } }, traverse: {} },
  },
  idWire: "legacy",
  extensions: {},
} satisfies ExtensionBinding<Contract, {}, {}>;

const client = connect<Contract, {}, {}>("https://api.example.test", null, binding);
const query = client.read({
  read: "Post",
  select: ["id", { comments: { select: ["id", "body"], limit: 2 } }],
  offset: 20,
});
void query;
void client.read({ read: "Post", select: ["id"], after: { id: 1 } });
void client.read({ read: "Post", select: ["id"], sort: [{ field: "id" }], after: { id: 1 } });
void client.read({ read: "Post", select: ["id"], sort: [{ field: "title" }], after: { title: "post", id: 1 } });

// @ts-expect-error child select is closed by the generated descriptor
client.read({ read: "Post", select: [{ comments: { select: ["secret"] } }] });
// @ts-expect-error resources without offset opt-in do not expose offset
client.read({ read: "Audit", select: ["id"], offset: 1 });
// @ts-expect-error resources without cursor opt-in do not expose after
client.read({ read: "Audit", select: ["id"], after: { id: 1 } });
// @ts-expect-error after values preserve the contract field type
client.read({ read: "Post", select: ["id"], after: { title: 1, id: 1 } });
// @ts-expect-error after keys are limited to id and allowed sort fields
client.read({ read: "Post", select: ["id"], after: { author: "hidden", id: 1 } });

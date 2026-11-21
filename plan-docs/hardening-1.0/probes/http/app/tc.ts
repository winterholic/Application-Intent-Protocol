import { connect } from "@aip/sdk";
import { contract } from "./binding.ts";
const client = connect("http://127.0.0.1:18471", null, contract);
// positive
await client.bundle({ atomic: [
  { create: "Order", values: { owner: "42", total: "1.00", status: "open" } },
  { create: "OrderLine", values: { product: "1", qty: 1, amount: "1.00" }, refs: { order: { created: 0 } } },
  { apply: "Order.confirm", target: { created: 0 } },
  { apply: "Product.take", target: { ids: ["1"] }, input: { qty: 1 } },
  { compose: "Order", targets: { ids: ["1"] }, steps: [{ transition: "confirm" }, { create: "Note", values: { owner: { item: "owner" } } }] },
] } as const, { key: "k" });
await client.bundle((b) => {
  const order = b.create("Order", { owner: "42", total: "1.00", status: "open" });
  b.create("OrderLine", { product: "1", qty: 1, amount: "1.00" }, { order });
  b.apply({ apply: "Product.take", target: { ids: ["1"] }, input: { qty: 1 } });
}, { key: "k2" });
await client.apply({ apply: "Product.restock", target: { ids: ["1"] }, input: { amount: "1.25" } }, { key: "k3" });
// negative (each must error)
// @ts-expect-error Decimal must be string
await client.apply({ apply: "Product.restock", target: { ids: ["1"] }, input: { amount: 1.25 } }, { key: "n1" });
// @ts-expect-error unknown transition
await client.apply({ apply: "Product.nope", target: { ids: ["1"] } }, { key: "n2" });
// @ts-expect-error input missing
await client.apply({ apply: "Product.take", target: { ids: ["1"] } }, { key: "n3" });
// @ts-expect-error refs wrong field
await client.bundle({ atomic: [{ create: "OrderLine", values: { product: "1", qty: 1, amount: "1.00" }, refs: { nope: { created: 0 } } }] } as const, { key: "n4" });
// @ts-expect-error unexposed create
await client.bundle({ atomic: [{ create: "Member", values: { name: "x" } }] } as const, { key: "n5" });

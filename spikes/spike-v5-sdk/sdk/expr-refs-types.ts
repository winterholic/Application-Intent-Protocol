// P0-A: the bundle builder and the raw `refs` / `target.created` forms accept exactly what the server exposes.
// Each `@ts-expect-error` line must be a real error. expr_refs_codegen.rs generates `./contract.ts` for its fixture definition,
// copies this file next to `generic.ts`, checks it compiles, then removes the markers and checks that tsc reports every one.
import type { BundleBuilder, BundleRequest, Created } from "./generic.ts";
import type { ApplyContract, CreateContract, Id } from "./contract.ts";

declare const b: BundleBuilder<ApplyContract, CreateContract>;
const me = "1" as Id<"Member">;
const someProduct = "5" as Id<"Product">;
const someOrder = "9" as Id<"Order">;

export function positive() {
  const order = b.create("Order", { owner: me, status: "open" });
  // the parent's id comes from the handle, so `order` is not in `values`; product is nullable and may be left out
  b.create("OrderLine", { owner: me, qty: 2 }, { order });
  b.create("OrderLine", { owner: me, qty: 1 }, { order, product: b.create("Product", { owner: me, stock: 3 }) });
  b.apply({ apply: "Order.confirm", target: order });
  b.apply({ apply: "Order.confirm", target: { ids: [someOrder] } });
  b.apply({ apply: "Product.take", target: { ids: [someProduct] }, input: { qty: 2 } });
  const typed: Created<"Order"> = order;
  void typed;
  // the raw wire form, symbols as the server stores them
  const raw: BundleRequest<ApplyContract, CreateContract> = {
    atomic: [
      { create: "Order", values: { owner: me, status: "open" } },
      { create: "OrderLine", values: { owner: me, qty: 1 }, refs: { order: { created: 0 } } },
      { apply: "Order.confirm", target: { created: 0 } },
    ],
  };
  void raw;
}

export function negatives() {
  const order = b.create("Order", { owner: me, status: "open" });
  const product = b.create("Product", { owner: me, stock: 3 });
  // @ts-expect-error a handle for a Product cannot fill the `order` reference
  b.create("OrderLine", { owner: me, qty: 1 }, { order: product });
  // @ts-expect-error `qty` is not a reference field
  b.create("OrderLine", { owner: me }, { qty: order });
  // @ts-expect-error `owner` is a Member reference and the handle is an Order
  b.create("OrderLine", { qty: 1 }, { order, owner: order });
  // @ts-expect-error a field comes from `values` or from `refs`, not both
  b.create("OrderLine", { owner: me, qty: 1, order: someOrder }, { order });
  // @ts-expect-error `qty` is still required
  b.create("OrderLine", { owner: me }, { order });
  // @ts-expect-error a plain number is not a handle
  b.create("OrderLine", { owner: me, qty: 1 }, { order: 0 });
  // @ts-expect-error a Product handle cannot be the target of an Order transition
  b.apply({ apply: "Order.confirm", target: product });
  // @ts-expect-error `sweep` only opens `where` targets, so a created row cannot be its target
  b.apply({ apply: "Order.sweep", target: order });
  // @ts-expect-error `Order.nothing` is not an exposed transition
  b.apply({ apply: "Order.nothing", target: order });
  const raw: BundleRequest<ApplyContract, CreateContract> = {
    // @ts-expect-error a symbol is `{ created: number }`, not a string
    atomic: [
      { create: "Order", values: { owner: me, status: "open" } },
      { create: "OrderLine", values: { owner: me, qty: 1 }, refs: { order: { created: "0" } } },
    ],
  };
  void raw;
}

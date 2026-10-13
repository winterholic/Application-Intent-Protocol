// V16: the generated bundle and compose types accept exactly what the server exposes. Each `@ts-expect-error` line
// must be a real error; v16_bundle.rs removes the markers and checks that tsc then reports them.
import { connectTypedApply } from "./typed.ts";
import { contract as stringContract, type Id as StringId, bundleLimits } from "./generated-v16-string.ts";
import { contract as safeContract, type Id as SafeId } from "./generated-v16-safe.ts";

const strings = connectTypedApply("http://type.invalid", "token", stringContract);
const numbers = connectTypedApply("http://type.invalid", "token", safeContract);

export async function typeCases() {
  const member = "1" as StringId<"Member">;
  const project = "10" as StringId<"Project">;
  const done = await strings.bundle({ atomic: [
    { create: "Task", values: { project, author: member, title: "write", points: 3 } },
    { create: "Log", values: { owner: member, name: "n" } },
    { apply: "Project.archive", target: { ids: [project] } },
  ] });
  if (done.ok) {
    const first = done.created[0];
    if (first.resource === "Task") { const id: StringId<"Task"> = first.id; void id; }
    if (first.resource === "Link") { const none: null = first.id; void none; }
    const changed: readonly StringId<"Project">[] = done.applied[0].changed;
    void changed;
  } else {
    const code: string = done.code;
    void code;
  }
  const limits: 20 = bundleLimits.maxOps; void limits;
  const number = await numbers.bundle({ atomic: [{ create: "Link", values: { project: 10 as SafeId<"Project">, member: 1 as SafeId<"Member"> } }] });
  void number;

  const composed = await strings.compose({ compose: "Project", targets: { ids: [project] }, steps: [
    { transition: "archive" },
    { create: "Log", values: { owner: { item: "owner" }, name: { item: "name" } } },
    { create: "Log", values: { owner: { item: "owner" }, name: { const: "fixed" }, note: { const: null } } },
  ] });
  if (composed.ok) { const log = composed.created[0]; const id: StringId<"Log"> = log.id; void id; }

  // a contract that exposes creates types an unsettled write as a single transition, a bundle or a compose
  for (const outcome of await strings.retryPending()) {
    const key: string = outcome.key;
    if (outcome.ok && "applied" in outcome) { const created = outcome.created; void created; }
    void key;
  }

  // @ts-expect-error a create that is not exposed
  await strings.bundle({ atomic: [{ create: "Member", values: {} }] });
  // @ts-expect-error a field the exposed create does not list
  await strings.bundle({ atomic: [{ create: "Task", values: { project, author: member, title: "x", points: 1, id: project } }] });
  // @ts-expect-error a required field is missing
  await strings.bundle({ atomic: [{ create: "Task", values: { title: "x" } }] });
  // @ts-expect-error a bundle needs at least one operation
  await strings.bundle({ atomic: [] });
  // @ts-expect-error a decimal-string Id is not a number
  await strings.bundle({ atomic: [{ create: "Task", values: { project: 10, author: member, title: "x", points: 1 } }] });
  // @ts-expect-error a transition that is not exposed
  await strings.bundle({ atomic: [{ apply: "Project.delete", target: { ids: [project] } }] });
  // @ts-expect-error a compose step the contract does not allow
  await strings.compose({ compose: "Project", targets: { ids: [project] }, steps: [{ transition: "delete" }] });
  // @ts-expect-error a value source the server did not declare
  await strings.compose({ compose: "Project", targets: { ids: [project] }, steps: [{ create: "Log", values: { owner: { item: "archived" }, name: { item: "name" } } }] });
  // @ts-expect-error a reference field takes no constant
  await strings.compose({ compose: "Project", targets: { ids: [project] }, steps: [{ create: "Log", values: { owner: { const: member }, name: { item: "name" } } }] });
  // @ts-expect-error a compose that is not exposed
  await strings.compose({ compose: "Task", targets: { ids: [project] }, steps: [{ transition: "archive" }] });
}

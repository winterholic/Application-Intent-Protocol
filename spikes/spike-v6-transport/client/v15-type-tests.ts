import { connectTypedApply } from "./typed.ts";
import { contract } from "./generated-v15-string.ts";
import { contract as numeric } from "./generated-v15-safe.ts";

function typesOnly() {
  const aip = connectTypedApply("http://unused", "unused", contract);
  aip.read({ read: "Event", select: ["id", "date"], filter: [{ field: "phase", op: "eq", value: "READY" }] });
  aip.apply({ apply: "Event.mark", target: { where: [{ field: "link", op: "eq", value: "local:thing" }, { field: "date", op: "eq", value: new Date().toISOString() }, { field: "phase", op: "eq", value: "DONE" }] } });
  // @ts-expect-error Enum membership remains closed
  aip.apply({ apply: "Event.mark", target: { where: [{ field: "phase", op: "eq", value: "UNKNOWN" }] } });
  // @ts-expect-error Url takes strings
  aip.apply({ apply: "Event.mark", target: { where: [{ field: "link", op: "eq", value: 1 }] } });
  // @ts-expect-error Time takes ISO strings
  aip.apply({ apply: "Event.mark", target: { where: [{ field: "date", op: "eq", value: new Date() }] } });
  // @ts-expect-error write target stays eq-only
  aip.apply({ apply: "Event.mark", target: { where: [{ field: "date", op: "gte", value: "2026-10-04T00:00:00Z" }] } });
  // @ts-expect-error read Enum membership remains closed
  aip.read({ read: "Event", select: ["id"], filter: [{ field: "phase", op: "eq", value: "UNKNOWN" }] });
  const safe = connectTypedApply("http://unused", "unused", numeric);
  safe.apply({ apply: "Event.mark", target: { where: [{ field: "link", op: "eq", value: "local:thing" }, { field: "date", op: "eq", value: new Date().toISOString() }, { field: "phase", op: "eq", value: "DONE" }] } });
  safe.apply({ apply: "Event.mark", target: { ids: [1] } });
  aip.apply({ apply: "Event.mark", target: { ids: ["1"] } });
  // @ts-expect-error numeric binding remains numeric
  safe.apply({ apply: "Event.mark", target: { ids: ["1"] } });
  // @ts-expect-error decimal binding remains string
  aip.apply({ apply: "Event.mark", target: { ids: [1] } });
  // @ts-expect-error numeric binding Enum membership also stays closed
  safe.apply({ apply: "Event.mark", target: { where: [{ field: "phase", op: "eq", value: "UNKNOWN" }] } });
}
void typesOnly;

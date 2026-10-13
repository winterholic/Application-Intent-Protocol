// A subscription is disabled after this many deliveries in a row failed. The count lives in the subscription row, so the
// policy is the application's: change the number, count other failures, or notify the owner when it disables.
const MAX_CONSECUTIVE_FAILURES = 3;

export async function fanout(input, ctx) {
  const event = { type: "order.paid", event: input.event };
  const page = await ctx.data.read({ read: "Subscription", select: ["id", "url", "active", "failures"] });
  let delivered = 0, failed = 0, disabled = 0;
  for (const subscription of page.rows.filter((row) => row.active)) {
    const key = `${input.event}-${subscription.id}`;
    try {
      await ctx.webhooks.deliver("subscribers", subscription.url, event, key);
      delivered++;
      // One success ends the streak.
      if (subscription.failures > 0) await ctx.data.apply("Subscription", "recovered", [subscription.id], `recovered-${key}`);
    } catch (error) {
      if (error.code !== "DELIVERY_FAILED" && error.code !== "DESTINATION_REJECTED") throw error;
      failed++;
      // A rejected destination (a private address, a malformed URL) never becomes valid by waiting; a failed delivery may.
      const hopeless = error.code === "DESTINATION_REJECTED" || subscription.failures + 1 >= MAX_CONSECUTIVE_FAILURES;
      await ctx.data.apply("Subscription", "failed", [subscription.id], `failed-${key}`);
      if (hopeless) {
        await ctx.data.apply("Subscription", "disable", [subscription.id], `disable-${key}`);
        disabled++;
      }
    }
  }
  return { delivered, failed, disabled };
}

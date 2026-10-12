export async function notify(input, ctx) {
  const event = { type: "order.paid", order: 11 };
  const sent = await ctx.webhooks.deliver("subscribers", input.url, event, "order-11-paid");
  // The same key replays the journaled delivery instead of sending again.
  const again = await ctx.webhooks.deliver("subscribers", input.url, event, "order-11-paid");
  const code = async (url, key) => {
    try {
      await ctx.webhooks.deliver("subscribers", url, event, key);
      return "sent";
    } catch (error) {
      return error.code;
    }
  };
  return {
    status: sent.status,
    id: sent.id,
    same: again.id === sent.id,
    privateCode: await code(input.private, "private"),
    failingCode: await code(input.failing, "failing"),
  };
}

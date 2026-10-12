export async function consume(input, ctx) {
  const message = JSON.parse(input.message);
  return { order: message.payload.order, deliveries: message.deliveries };
}

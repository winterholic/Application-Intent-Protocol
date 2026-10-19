export function count(input) { return {length:[...input.text].length}; }
export async function slow() { await new Promise(r => setTimeout(r, 5000)); return {length:1}; }
export async function report(input, ctx) {
  const result = await ctx.jobs.run("length", input, "measure");
  const file = await ctx.files.put("reports", {name:"report.txt",mediaType:"text/plain",base64:btoa(input.text)}, "report-file");
  await ctx.events.publish("notifications", "report-ready", {file:file.id,length:result.length});
  return {file:file.id,length:result.length};
}
export async function httpEcho(input, ctx) {
  const response = await ctx.http.call("echo", {text: input.text}, "charge");
  return {length: [...response.body.text].length, status: response.status};
}
export async function httpEchoReplay(input, ctx) {
  await ctx.http.call("echo", {text: input.text}, "charge");
  await ctx.jobs.run("length", input, "measure");
  const response = await ctx.http.call("echo", {text: input.text}, "charge");
  return {length: [...response.body.text].length, status: response.status};
}
export async function httpHold(input, ctx) {
  const response = await ctx.http.call("hold", {text: input.text}, "hold-once");
  return {length: [...response.body.text].length, status: response.status};
}
export async function httpBroken(input, ctx) {
  await ctx.http.call("broken", {text: input.text}, "once");
  return {ok: true};
}
export async function httpBrokenCaught(input, ctx) {
  try {
    await ctx.http.call("broken", {text: input.text}, "once");
  } catch (error) {
    return {ok: false};
  }
  return {ok: true};
}
export async function budgetRoot(input, ctx) {
  return await ctx.jobs.run("budgetChild", {}, "budget-child");
}
export async function budgetChild(input, ctx) {
  for (let index = 0; index < 64; index++) {
    await ctx.events.publish("notifications", `budget-${index}`, {index});
  }
  return {length: 64};
}
export async function knownFailChild() {
  return {length: "wrong-type"};
}
export async function catchKnownChildFailure(input, ctx) {
  let recovered = 0;
  for (let index = 0; index < 2; index++) {
    try {
      await ctx.jobs.run("knownFailChild", {}, "known-failure");
    } catch (error) {
      if (error?.code !== "WIRE.STEP_FAILED") throw error;
      recovered++;
    }
  }
  return {length: recovered};
}

export async function message(input, ctx) {
  await ctx.messages.publish("notifications", {text:input.text}, "dispatch");
  await ctx.messages.publish("notifications", {text:input.text}, "dispatch");
  return {length:[...input.text].length};
}

export async function traceRoot(input, ctx) {
  return await ctx.jobs.run("traceMiddle", input, "middle");
}
export async function traceMiddle(input, ctx) {
  return await ctx.jobs.run("count", input, "leaf");
}

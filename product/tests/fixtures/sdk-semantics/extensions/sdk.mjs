// 쓰기 확장: 입력 Decimal을 입금 전이로 위임하고 Uuid id·Decimal·nullable 출력을 돌려준다. fail이면 업무 거절(ctx.reject)이다.
export async function adjust(input, ctx) {
  if (input.fail) throw ctx.reject("NOT_TODAY", "오늘은 안 됩니다");
  await ctx.data.apply("Account", "deposit", [input.a], undefined, undefined, undefined, { amount: input.amount });
  return { id: input.a, moved: input.amount, note: null };
}

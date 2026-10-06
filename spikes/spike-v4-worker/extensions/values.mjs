export async function echo(input) {
  return { value: input.value };
}

export async function nul() {
  return { value: "left\u0000right" };
}

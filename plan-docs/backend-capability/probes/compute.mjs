export async function count(input) { return { length: Array.from(input.text).length }; }
export async function slow(input) { await new Promise(resolve => setTimeout(resolve, 6000)); return count(input); }

const composeHashtagPattern =
  /(?:^|[^\p{L}\p{M}\p{N}_/#?&=])#([\p{L}\p{M}\p{N}_]+)/gu;

export function retainedComposeHashtags(text: string): string {
  return Array.from(text.matchAll(composeHashtagPattern), (match) => `#${match[1]}`)
    .join(" ");
}

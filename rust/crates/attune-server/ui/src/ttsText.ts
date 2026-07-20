/** Match the Scheduler TTS contract: trim only outer ASCII U+0020 spaces. */
export function trimOuterAsciiSpaces(text: string): string {
  let start = 0;
  let end = text.length;
  while (start < end && text.charCodeAt(start) === 0x20) start += 1;
  while (end > start && text.charCodeAt(end - 1) === 0x20) end -= 1;
  return text.slice(start, end);
}

const MAX_TTS_TEXT_SCALARS = 128;

/** Return the exact public TTS request text, or null for deterministic 4xx input. */
export function ttsRequestText(text: string): string | null {
  for (const scalar of text) {
    const codePoint = scalar.codePointAt(0)!;
    if (codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f)) return null;
  }
  const trimmed = trimOuterAsciiSpaces(text);
  if (!trimmed || [...trimmed].length > MAX_TTS_TEXT_SCALARS) return null;
  return trimmed;
}

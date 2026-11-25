// The API returns descending (started_ms, id) pages. Reverse the page, not
// the sentences within each clip, and preserve the server's timestamp tie-break.
export function chronologicalPage(items) {
  return [...items].reverse();
}

export function prependOlderClips(current, items) {
  const seen = new Set(current.map(clip => clip.id));
  const older = chronologicalPage(items).filter(clip => {
    if (seen.has(clip.id)) return false;
    seen.add(clip.id);
    return true;
  });
  return [...older, ...current];
}

export function scrollIntent(previousTop, { scrollTop, scrollHeight, clientHeight }) {
  const movingUp = scrollTop < previousTop;
  return {
    browse: movingUp && scrollHeight - clientHeight - scrollTop > 48,
    older: movingUp && scrollTop <= 72,
  };
}

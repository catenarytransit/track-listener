// Keep both region and channel order as configured by the operator.
export function groupChannels(channels) {
  const groups = new Map();
  for (const channel of channels) {
    const name = channel.region?.trim() || 'Unassigned';
    if (!groups.has(name)) groups.set(name, []);
    groups.get(name).push(channel);
  }
  return Array.from(groups, ([name, channels]) => ({ name, channels }));
}

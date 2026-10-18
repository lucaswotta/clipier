/** "45s", "12min" or, when `precise`, "12min 5s"; hours above 60 minutes. */
export function formatDuration(totalSeconds, precise = false) {
  const seconds = Math.max(0, Math.round(totalSeconds));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return precise && seconds % 60 ? `${minutes}min ${seconds % 60}s` : `${minutes}min`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}min`;
}

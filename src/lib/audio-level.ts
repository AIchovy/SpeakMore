// Display RMS on a fixed dBFS scale. Linear amplitude compresses quiet speech
// into the first segment; a fixed scale also keeps silence and noise meaningful.
export function audioLevelFraction(rms: number): number {
  if (!Number.isFinite(rms) || rms <= 0) return 0
  const db = 20 * Math.log10(rms)
  const floorDb = -60
  const ceilingDb = -6
  return Math.min(1, Math.max(0, (db - floorDb) / (ceilingDb - floorDb)))
}

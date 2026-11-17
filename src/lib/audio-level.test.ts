import { describe, expect, it } from 'vitest'
import { audioLevelFraction } from './audio-level'

const segments = (rms: number) => Math.ceil(audioLevelFraction(rms) * 8)

describe('recording level display', () => {
  it('keeps silence, the noise floor and invalid samples unlit', () => {
    for (const rms of [0, 0.0001, 0.001, -1, NaN, Infinity]) {
      expect(segments(rms)).toBe(0)
    }
  })

  it('distinguishes low speech amplitudes that all occupied the first linear segment', () => {
    expect(segments(0.003)).toBe(2)
    expect(segments(0.01)).toBe(3)
    expect(segments(0.03)).toBe(5)
    expect(segments(0.06)).toBe(6)
  })

  it('fills but never exceeds eight segments for loud signals', () => {
    for (const rms of [0.5, 1, 2]) expect(segments(rms)).toBe(8)
  })
})

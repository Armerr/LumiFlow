import { describe, expect, test } from 'vitest'
import { getActiveFanIndex, getFanCardMetrics, getFanCardPose, getFanPosterStats } from './fanLayout'

describe('fan layout', () => {
  test('sizes album cards so multiple cards fit in the viewport', () => {
    const metrics = getFanCardMetrics(
      { width: 1440, height: 900 },
      { width: 26.5, height: 16.56 },
    )

    expect(metrics.width).toBeLessThan(26.5 / 4)
    expect(metrics.height).toBeLessThan(16.56 * 0.55)
    expect(metrics.slot).toBeGreaterThan(metrics.width)
  })

  test('keeps circular-gallery arc positions inside the visible camera range', () => {
    const center = getFanCardPose(0, 26.5)
    const left = getFanCardPose(-10, 26.5)
    const right = getFanCardPose(10, 26.5)

    expect(center.y).toBeGreaterThan(-1)
    expect(left.y).toBeGreaterThan(-2)
    expect(right.y).toBeGreaterThan(-2)
    expect(Math.abs(left.rotationZ)).toBeLessThan(0.25)
    expect(Math.abs(right.rotationZ)).toBeLessThan(0.25)
  })

  test('uses a non-overlapping shelf on portrait mobile', () => {
    const metrics = getFanCardMetrics(
      { width: 390, height: 844 },
      { width: 7.66, height: 16.56 },
    )

    expect(metrics.slot).toBeGreaterThan(metrics.width * 1.22)
    expect(metrics.width).toBeGreaterThan(7.66 * 0.4)
  })

  test('places portrait mobile cards in a compact mid-screen shelf', () => {
    const center = getFanCardPose(0, 7.66, 16.56)
    const side = getFanCardPose(3.8, 7.66, 16.56)

    expect(center.y).toBeGreaterThan(-0.9)
    expect(center.y).toBeLessThan(-0.15)
    expect(Math.abs(side.rotationZ)).toBeLessThan(0.06)
  })

  test('selects the card closest to screen center as active', () => {
    expect(getActiveFanIndex([6, -1.2, 2.4, 0.35])).toBe(3)
    expect(getActiveFanIndex([])).toBe(-1)
  })

  test('summarizes albums for poster chrome', () => {
    expect(getFanPosterStats([6, 8, 10])).toEqual({ albumCount: 3, photoCount: 24 })
    expect(getFanPosterStats([])).toEqual({ albumCount: 0, photoCount: 0 })
  })
})

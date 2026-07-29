import { describe, expect, test } from 'vitest'
import {
  getGridColumnCount,
  getPageOffsets,
  getPhotoPosition,
  getVerticalGridMetrics,
  getVisiblePhotoRange,
  PHOTO_PAGE_SIZE,
} from './gridLayout'

describe('vertical photo grid', () => {
  test('uses fixed responsive columns without a horizontal overflow axis', () => {
    expect(getGridColumnCount(390)).toBe(2)
    expect(getGridColumnCount(800)).toBe(3)
    expect(getGridColumnCount(1280)).toBe(4)
    expect(getGridColumnCount(1900)).toBe(5)

    const metrics = getVerticalGridMetrics(390, 4)
    expect(metrics.itemWidth * metrics.columns + metrics.gap * (metrics.columns - 1)).toBeCloseTo(390)
    expect(metrics.totalHeight).toBeGreaterThan(0)
  })

  test('renders only visible rows plus bounded overscan', () => {
    const metrics = getVerticalGridMetrics(1200, 4750)
    const first = getVisiblePhotoRange(0, 900, metrics, 4750)
    const middle = getVisiblePhotoRange(metrics.rowStride * 300, 900, metrics, 4750)

    expect(first.start).toBe(0)
    expect(first.end - first.start).toBeLessThanOrEqual(32)
    expect(middle.start).toBeGreaterThan(1000)
    expect(middle.end - middle.start).toBeLessThanOrEqual(40)
  })

  test('requests only pages intersecting the virtualized window', () => {
    expect(getPageOffsets({ start: 0, end: 24 }, PHOTO_PAGE_SIZE)).toEqual([0])
    expect(getPageOffsets({ start: 58, end: 82 }, PHOTO_PAGE_SIZE)).toEqual([0, 60])
    expect(getPageOffsets({ start: 121, end: 145 }, PHOTO_PAGE_SIZE)).toEqual([120])
  })

  test('positions every photo on the vertical axis in a fixed grid', () => {
    const metrics = getVerticalGridMetrics(1200, 100)
    expect(getPhotoPosition(0, metrics)).toMatchObject({ left: 0, top: 0 })
    expect(getPhotoPosition(3, metrics).top).toBe(0)
    expect(getPhotoPosition(4, metrics).top).toBe(metrics.rowStride)
  })
})

import { describe, expect, test } from 'vitest'
import { buildInfiniteGridItems, getContainedPhotoSize, getGridColumnCount, getGridFieldSize, getGridMotionMultiplier, getGridPlaneInset, resolveGridHitSourceIndex, REFERENCE_GRID_ITEM_COUNT } from './gridLayout'
import type { Photo } from '../../shared/types'

const photo = (id: number): Photo => ({
  id,
  name: `photo-${id}.jpg`,
  width: 1200,
  height: 800,
  size_bytes: 1024,
  format: 'jpg',
})

describe('grid layout', () => {
  test('repeats sparse albums to match the original 15-plane reference grid', () => {
    const items = buildInfiniteGridItems([photo(0), photo(1), photo(2)], REFERENCE_GRID_ITEM_COUNT)

    expect(items).toHaveLength(15)
    expect(items.map((item) => item.photo.id)).toEqual([0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2])
    expect(items.map((item) => item.sourceIndex)).toEqual([0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2])
    expect(items.at(-1)).toMatchObject({ key: '14-2', sourceIndex: 2 })
  })

  test('keeps albums above the reference field size one-to-one for photo navigation', () => {
    const photos = Array.from({ length: 18 }, (_, id) => photo(id))
    const items = buildInfiniteGridItems(photos, REFERENCE_GRID_ITEM_COUNT)

    expect(items).toHaveLength(18)
    expect(items[17]).toMatchObject({ photo: photos[17], sourceIndex: 17, key: '17-17' })
  })

  test('uses responsive columns but keeps motion tied to the rendered column', () => {
    expect(getGridColumnCount(390)).toBe(3)
    expect(getGridColumnCount(900)).toBe(4)
    expect(getGridColumnCount(1280)).toBe(5)
    expect(getGridColumnCount(1900)).toBe(6)
    expect([0, 1, 2, 3, 4, 5].map((i) => getGridMotionMultiplier(i, 3))).toEqual([1, 0.9, 0.8, 1, 0.9, 0.8])
  })

  test('fills grid cells while leaving a small gutter for every photo shape', () => {
    expect(getContainedPhotoSize({ width: 1200, height: 800 }, { width: 410, height: 285 }, 4)).toEqual({ width: 402, height: 277 })
    expect(getContainedPhotoSize({ width: 800, height: 1200 }, { width: 410, height: 285 }, 4)).toEqual({ width: 402, height: 277 })
    expect(getContainedPhotoSize({ width: 1200, height: 1200 }, { width: 410, height: 285 }, 4)).toEqual({ width: 402, height: 277 })
  })

  test('keeps only a small gap around all photos in the desktop grid', () => {
    const viewport = { width: 1280, height: 900 }
    const columns = getGridColumnCount(viewport.width)
    const rows = REFERENCE_GRID_ITEM_COUNT / columns
    const field = getGridFieldSize(viewport)
    const cell = { width: field.width / columns, height: field.height / rows }
    const inset = getGridPlaneInset(viewport.width)

    for (const image of [{ width: 1200, height: 800 }, { width: 800, height: 1200 }, { width: 1200, height: 1200 }]) {
      const size = getContainedPhotoSize(image, cell, inset)
      expect(Math.round(cell.width - size.width)).toBeLessThanOrEqual(10)
      expect(Math.round(cell.height - size.height)).toBeLessThanOrEqual(10)
    }
  })

  test('maps the rendered plane hit back to its real photo index', () => {
    const items = buildInfiniteGridItems([photo(0), photo(1), photo(2)], REFERENCE_GRID_ITEM_COUNT)
    const hits = [{ object: { userData: { gridItemIndex: 13 } } }]

    expect(resolveGridHitSourceIndex(hits, items)).toBe(1)
    expect(resolveGridHitSourceIndex([], items)).toBeNull()
    expect(resolveGridHitSourceIndex([{ object: { userData: {} } }], items)).toBeNull()
  })
})

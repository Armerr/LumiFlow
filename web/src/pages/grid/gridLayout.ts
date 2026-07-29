import type { Photo } from '../../shared/types'

export const REFERENCE_GRID_COLUMNS = 5
export const REFERENCE_GRID_ITEM_COUNT = 15

export interface InfiniteGridItem {
  key: string
  photo: Photo
  sourceIndex: number
}

export function getGridColumnCount(width: number): number {
  if (width < 640) return 3
  if (width < 1100) return 4
  if (width < 1800) return REFERENCE_GRID_COLUMNS
  return 6
}

export function getGridMotionMultiplier(index: number, columns: number): number {
  return 1 - (index % columns) * 0.1
}

export function getGridFieldSize(viewport: Size2D): Size2D {
  return {
    width: Math.round(viewport.width * 1.6),
    height: Math.round(viewport.height * 0.95),
  }
}

export function getGridPlaneInset(viewportWidth: number): number {
  return Math.max(4, Math.round(viewportWidth * 0.003))
}

export interface Size2D {
  width: number
  height: number
}

export function getContainedPhotoSize(_image: Size2D, cell: Size2D, inset: number): Size2D {
  return {
    width: Math.round(Math.max(cell.width - inset * 2, 1)),
    height: Math.round(Math.max(cell.height - inset * 2, 1)),
  }
}

export function buildInfiniteGridItems(photos: Photo[], minItems: number): InfiniteGridItem[] {
  if (photos.length === 0) return []

  const itemCount = Math.max(photos.length, minItems)
  const items: InfiniteGridItem[] = []

  for (let slot = 0; slot < itemCount; slot += 1) {
    const sourceIndex = slot % photos.length
    items.push({
      key: `${slot}-${sourceIndex}`,
      photo: photos[sourceIndex],
      sourceIndex,
    })
  }

  return items
}

export interface GridRaycastHit {
  object: { userData: Record<string, unknown> }
}

export function resolveGridHitSourceIndex(
  hits: GridRaycastHit[],
  items: InfiniteGridItem[],
): number | null {
  const itemIndex = hits[0]?.object.userData.gridItemIndex
  if (typeof itemIndex !== 'number') return null
  return items[itemIndex]?.sourceIndex ?? null
}

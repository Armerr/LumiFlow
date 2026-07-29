export const PHOTO_PAGE_SIZE = 60
const GRID_GAP = 6
const ITEM_ASPECT_RATIO = 4 / 3
const OVERSCAN_ROWS = 2

export interface VerticalGridMetrics {
  columns: number
  gap: number
  itemWidth: number
  itemHeight: number
  rowStride: number
  totalHeight: number
}

export interface PhotoRange {
  start: number
  end: number
}

export function getGridColumnCount(width: number): number {
  if (width < 640) return 2
  if (width < 1000) return 3
  if (width < 1600) return 4
  return 5
}

export function getVerticalGridMetrics(width: number, photoCount: number): VerticalGridMetrics {
  const columns = getGridColumnCount(width)
  const gap = GRID_GAP
  const itemWidth = Math.max((width - gap * (columns - 1)) / columns, 1)
  const itemHeight = itemWidth / ITEM_ASPECT_RATIO
  const rowStride = itemHeight + gap
  const rows = Math.ceil(photoCount / columns)
  return {
    columns,
    gap,
    itemWidth,
    itemHeight,
    rowStride,
    totalHeight: rows === 0 ? 0 : rows * itemHeight + (rows - 1) * gap,
  }
}

export function getVisiblePhotoRange(
  scrollTop: number,
  viewportHeight: number,
  metrics: VerticalGridMetrics,
  photoCount: number,
): PhotoRange {
  if (photoCount === 0) return { start: 0, end: 0 }
  const firstRow = Math.max(Math.floor(scrollTop / metrics.rowStride) - OVERSCAN_ROWS, 0)
  const lastRow = Math.ceil((scrollTop + viewportHeight) / metrics.rowStride) + OVERSCAN_ROWS
  return {
    start: firstRow * metrics.columns,
    end: Math.min(lastRow * metrics.columns, photoCount),
  }
}

export function getPageOffsets(range: PhotoRange, pageSize = PHOTO_PAGE_SIZE): number[] {
  if (range.end <= range.start) return []
  const firstPage = Math.floor(range.start / pageSize) * pageSize
  const lastPage = Math.floor((range.end - 1) / pageSize) * pageSize
  const offsets: number[] = []
  for (let offset = firstPage; offset <= lastPage; offset += pageSize) offsets.push(offset)
  return offsets
}

export function getPhotoPosition(index: number, metrics: VerticalGridMetrics): { left: number; top: number } {
  return {
    left: (index % metrics.columns) * (metrics.itemWidth + metrics.gap),
    top: Math.floor(index / metrics.columns) * metrics.rowStride,
  }
}

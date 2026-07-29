import { afterEach, describe, expect, test, vi } from 'vitest'
import { api } from '../../shared/api'
import { VerticalPhotoGrid } from './VerticalPhotoGrid'

interface GridInternals {
  wantedPages: Set<number>
  render: () => void
  loading: Set<number>
  loadPages: (offsets: number[]) => Promise<void>
}

function createGridInternals(): GridInternals {
  const scroller = {} as HTMLElement
  const stage = { clientWidth: 390 } as HTMLElement
  const grid = new VerticalPhotoGrid(scroller, stage, 'album-id', 120, [])
  return grid as unknown as GridInternals
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe('vertical grid page loading', () => {
  test('does not retry a failed page until another scroll or resize', async () => {
    vi.spyOn(api, 'album').mockRejectedValue(new Error('offline'))
    const grid = createGridInternals()
    grid.wantedPages = new Set([60])
    grid.render = vi.fn()

    await grid.loadPages([60])

    expect(api.album).toHaveBeenCalledTimes(1)
    expect(grid.render).not.toHaveBeenCalled()
  })

  test('renders after a currently wanted page loads successfully', async () => {
    vi.spyOn(api, 'album').mockResolvedValue({ name: 'Album', photo_count: 120, photos: [] })
    const grid = createGridInternals()
    grid.wantedPages = new Set([60])
    grid.render = vi.fn(() => expect(grid.loading.has(60)).toBe(false))

    await grid.loadPages([60])

    expect(grid.render).toHaveBeenCalledTimes(1)
  })
})

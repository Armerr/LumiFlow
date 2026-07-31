import { afterEach, describe, expect, test, vi } from 'vitest'
import { albumNavigationIdentity, createFanPage, renderScanStatus } from './fan'
import { api } from '../shared/api'

afterEach(() => {
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe('fan album navigation identity', () => {
  test('uses timeline album ID instead of its display name', () => {
    expect(albumNavigationIdentity({
      id: 'day:2026-06-02',
      name: '2026年6月2日 · 上海',
      description: '一天的回忆',
      photo_count: 3,
    })).toBe('day:2026-06-02')
  })

  test('falls back to folder album name', () => {
    expect(albumNavigationIdentity({
      name: 'Family / 2025',
      cover: 'cover.jpg',
      count: 3,
      created_at: '2025-01-01T00:00:00Z',
      updated_at: '2025-01-01T00:00:00Z',
    })).toBe('Family / 2025')
  })
})

describe('startup scan screen', () => {
  test('renders live progress and a clear waiting message', () => {
    const html = renderScanStatus({
      state: 'scanning',
      phase: 'indexing',
      found: 125,
      processed: 120,
      errors: 1,
      workers: 4,
      elapsed_seconds: 3,
    })

    expect(html).toContain('正在同步照片库')
    expect(html).toContain('已发现 125 张')
    expect(html).toContain('已校验 120 张')
    expect(html).toContain('后续同步会复用未变化照片的数据')
  })
})

describe('startup scan polling', () => {
  test('automatically loads albums after the scan becomes ready', async () => {
    vi.useFakeTimers()
    vi.spyOn(api, 'status')
      .mockResolvedValueOnce({ state: 'scanning', phase: 'indexing', found: 10, processed: 4, errors: 0, workers: 4, elapsed_seconds: 1 })
      .mockResolvedValueOnce({ state: 'ready', phase: 'ready', found: 10, processed: 10, errors: 0, workers: 4, elapsed_seconds: 2 })
    vi.spyOn(api, 'albums').mockResolvedValue({ albums: [], updated: 'now' })
    const container = { innerHTML: '' } as HTMLElement
    const page = createFanPage()

    await page.mount(container)
    expect(container.innerHTML).toContain('正在同步照片库')
    await vi.advanceTimersByTimeAsync(1000)

    expect(api.albums).toHaveBeenCalledTimes(1)
    expect(container.innerHTML).toContain('暂无相册')
    page.unmount()
  })
})

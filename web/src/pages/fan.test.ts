import { describe, expect, test } from 'vitest'
import { albumNavigationIdentity } from './fan'

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

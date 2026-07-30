import { describe, expect, test } from 'vitest'
import { albumPresentation } from './albumPresentation'

describe('album presentation', () => {
  test('uses timeline date, place, and AI summary instead of the folder-derived name', () => {
    expect(albumPresentation({
      name: 'Camera Uploads · 2026-07-28 东京 · 夏令节',
      date_start: '2026-07-28',
      place: '东京',
      description: '午后的步行、街景与短暂停留。',
      photo_count: 10,
    })).toEqual({
      metadata: '2026.07.28 · 东京',
      summary: '午后的步行、街景与短暂停留。',
    })
  })

  test('uses a neutral photo-count summary when AI text or a place is unavailable', () => {
    expect(albumPresentation({
      name: 'Legacy Folder · 2026-07-28',
      date_start: '2026-07-28',
      photo_count: 12,
    })).toEqual({
      metadata: '2026.07.28',
      summary: '12 张照片',
    })
  })

  test('keeps the folder name only when no timeline metadata exists', () => {
    expect(albumPresentation({
      name: '2025-京都夜色',
      count: 9,
    })).toEqual({
      metadata: '2025-京都夜色',
      summary: '9 张照片',
    })
  })
})

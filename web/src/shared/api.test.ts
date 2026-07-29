import { afterEach, describe, expect, test, vi } from 'vitest'
import { api } from './api'
import type { Photo } from './types'

const timelinePhoto: Photo = {
  id: 'sha1-photo-id',
  name: 'nested photo.jpg',
  relative_path: 'Trip/nested photo.jpg',
  width: 1600,
  height: 900,
  size_bytes: 1234,
  format: 'jpg',
}

const folderPhoto: Photo = {
  id: 4,
  name: 'legacy photo.jpg',
  width: 1600,
  height: 900,
  size_bytes: 1234,
  format: 'jpg',
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('photo media URL contracts', () => {
  test('uses database by-ID routes for timeline photos', () => {
    expect(api.thumbUrl('ignored-album', timelinePhoto)).toBe('/api/thumbs/by-id/sha1-photo-id')
    expect(api.photoUrl('ignored-album', timelinePhoto)).toBe('/api/photos/by-id/sha1-photo-id')
  })

  test('uses encoded album and filename routes for folder photos', () => {
    expect(api.thumbUrl('Family / 2025', folderPhoto)).toBe('/api/thumbs/Family%20%2F%202025/legacy%20photo.jpg')
    expect(api.photoUrl('Family / 2025', folderPhoto)).toBe('/api/photos/Family%20%2F%202025/legacy%20photo.jpg')
  })
})

describe('EXIF URL contracts', () => {
  test('fetches timeline EXIF by photo ID', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: () => Promise.resolve({}) })
    vi.stubGlobal('fetch', fetchMock)

    await api.exif('ignored-album', timelinePhoto)

    expect(fetchMock).toHaveBeenCalledWith('/api/exif/by-id/sha1-photo-id')
  })

  test('fetches folder EXIF by encoded album and filename', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: () => Promise.resolve({}) })
    vi.stubGlobal('fetch', fetchMock)

    await api.exif('Family / 2025', folderPhoto)

    expect(fetchMock).toHaveBeenCalledWith('/api/exif/Family%20%2F%202025/legacy%20photo.jpg')
  })
})

describe('startup status', () => {
  test('fetches live initial scan status', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: () => Promise.resolve({ state: 'scanning', phase: 'indexing', found: 125, processed: 120, errors: 1, elapsed_seconds: 3 }) })
    vi.stubGlobal('fetch', fetchMock)

    await api.status()

    expect(fetchMock).toHaveBeenCalledWith('/api/status')
  })
})

describe('album pagination', () => {
  test('fetches only the requested vertical-grid page', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: () => Promise.resolve({ name: 'Trip', photo_count: 4750, photos: [] }) })
    vi.stubGlobal('fetch', fetchMock)

    await api.album('day:2025-01-02', 120, 60)

    expect(fetchMock).toHaveBeenCalledWith('/api/albums/day%3A2025-01-02?offset=120&limit=60')
  })
})

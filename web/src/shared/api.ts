import type { AlbumsResponse, AlbumDetail, ExifData } from './types'

const BASE = ''

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(BASE + url)
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`)
  return res.json()
}

export const api = {
  /** GET /api/albums */
  albums(): Promise<AlbumsResponse> {
    return fetchJson('/api/albums')
  },

  /** GET /api/albums/:name */
  album(name: string): Promise<AlbumDetail> {
    return fetchJson(`/api/albums/${encodeURIComponent(name)}`)
  },

  /** GET /api/exif/:album/:file */
  exif(album: string, file: string): Promise<ExifData> {
    return fetchJson(`/api/exif/${encodeURIComponent(album)}/${encodeURIComponent(file)}`)
  },

  /** Construct URL for a thumbnail. */
  thumbUrl(album: string, file: string): string {
    return `/api/thumbs/${encodeURIComponent(album)}/${encodeURIComponent(file)}`
  },

  /** Construct URL for the original photo. */
  photoUrl(album: string, file: string): string {
    return `/api/photos/${encodeURIComponent(album)}/${encodeURIComponent(file)}`
  },
}

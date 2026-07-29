import type { AlbumsResponse, AlbumDetail, ExifData, Photo, ScanStatus } from './types'

const BASE = ''

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(BASE + url)
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`)
  return res.json()
}

function mediaUrl(kind: 'exif' | 'thumbs' | 'photos', albumId: string, photo: Pick<Photo, 'id' | 'name'>): string {
  if (typeof photo.id === 'string') {
    return `/api/${kind}/by-id/${encodeURIComponent(photo.id)}`
  }

  return `/api/${kind}/${encodeURIComponent(albumId)}/${encodeURIComponent(photo.name)}`
}

export const api = {
  /** GET current startup/index status. */
  status(): Promise<ScanStatus> {
    return fetchJson('/api/status')
  },

  /** GET /api/albums */
  albums(): Promise<AlbumsResponse> {
    return fetchJson('/api/albums')
  },

  /** GET one page from /api/albums/:id. */
  album(albumId: string, offset = 0, limit = 60): Promise<AlbumDetail> {
    const query = new URLSearchParams({ offset: String(offset), limit: String(limit) })
    return fetchJson(`/api/albums/${encodeURIComponent(albumId)}?${query}`)
  },

  /** Fetch EXIF through the route matching the photo identity kind. */
  exif(albumId: string, photo: Pick<Photo, 'id' | 'name'>): Promise<ExifData> {
    return fetchJson(mediaUrl('exif', albumId, photo))
  },

  /** Construct a thumbnail URL through the route matching the photo identity kind. */
  thumbUrl(albumId: string, photo: Pick<Photo, 'id' | 'name'>): string {
    return mediaUrl('thumbs', albumId, photo)
  },

  /** Construct an original URL through the route matching the photo identity kind. */
  photoUrl(albumId: string, photo: Pick<Photo, 'id' | 'name'>): string {
    return mediaUrl('photos', albumId, photo)
  },
}

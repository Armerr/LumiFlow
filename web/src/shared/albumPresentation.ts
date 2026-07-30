import type { Album } from './types'

export interface AlbumPresentation {
  metadata: string
  summary: string
}

export function albumPresentation(album: Pick<Album, 'name' | 'date_start' | 'place' | 'description' | 'photo_count' | 'count'>): AlbumPresentation {
  const date = formatAlbumDate(album.date_start)
  const place = album.place?.trim()
  const metadata = [date, place].filter(Boolean).join(' · ') || album.name
  const photoCount = album.photo_count ?? album.count ?? 0
  const description = album.description?.trim()

  return {
    metadata,
    summary: description || `${photoCount} 张照片`,
  }
}

function formatAlbumDate(value: string | null | undefined): string {
  if (!value) return ''
  const match = /^(\d{4})-(\d{2})-(\d{2})/.exec(value)
  return match ? `${match[1]}.${match[2]}.${match[3]}` : value
}

export interface AlbumFilter {
  person?: string
  from?: string
  to?: string
}

export function albumFilterQuery(filter: AlbumFilter): string {
  const params = new URLSearchParams()
  if (filter.person) params.set('person', filter.person)
  if (filter.from) params.set('from', filter.from)
  if (filter.to) params.set('to', filter.to)
  return params.toString()
}

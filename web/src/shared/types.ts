/** API response types — mirrors the Rust backend. */

export interface Album {
  name: string
  id?: string
  description?: string | null
  date_start?: string | null
  date_end?: string | null
  place?: string | null
  holiday?: string | null
  cover_photo_id?: string | null
  photo_count?: number
  cover?: string
  count?: number
  created_at?: string
  updated_at?: string
}

export interface AlbumsResponse {
  albums: Album[]
  updated: string
}

export interface ScanStatus {
  state: 'starting' | 'scanning' | 'ready' | 'error'
  phase: string
  found: number
  processed: number
  errors: number
  workers: number
  elapsed_seconds: number
  error?: string | null
}

export interface Photo {
  id: number | string
  name: string
  width: number
  height: number
  size_bytes: number
  format: string
  relative_path?: string
  taken_at?: string | null
  time_source?: string
}

export interface AlbumDetail {
  name: string
  id?: string
  description?: string | null
  date_start?: string | null
  date_end?: string | null
  place?: string | null
  holiday?: string | null
  cover_photo_id?: string | null
  photo_count?: number
  photos: Photo[]
}

export interface GpsCoords {
  lat: number
  lon: number
}

export interface ImageDimensions {
  width: number
  height: number
}

export interface ToneAnalysis {
  type: string
  brightness: number
  contrast: number
  shadows: number
  highlights: number
}

export interface ExifData {
  make: string | null
  model: string | null
  lens: string | null
  focal_length: string | null
  aperture: string | null
  shutter_speed: string | null
  iso: number | null
  date_taken: string | null
  timezone: string | null
  gps: GpsCoords | null
  dimensions: ImageDimensions
  file_size: number
  format: string
  flash: string | null
  software: string | null
  orientation: number | null
  artist: string | null
  color_space: string | null
  image_description: string | null
  user_comment: string | null
  tags: string[]
  tone: ToneAnalysis | null
}

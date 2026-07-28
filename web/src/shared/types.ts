/** API response types — mirrors the Rust backend. */

export interface Album {
  name: string
  cover: string
  count: number
  created_at: string
  updated_at: string
}

export interface AlbumsResponse {
  albums: Album[]
  updated: string
}

export interface Photo {
  id: number
  name: string
  width: number
  height: number
  size_bytes: number
  format: string
}

export interface AlbumDetail {
  name: string
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

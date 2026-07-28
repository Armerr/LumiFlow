export interface Size2D {
  width: number
  height: number
}

export interface FanCardMetrics {
  width: number
  height: number
  slot: number
}

export interface FanCardPose {
  x: number
  y: number
  rotationZ: number
}

export function getFanCardMetrics(screen: Size2D, viewport: Size2D): FanCardMetrics {
  const aspect = screen.width / Math.max(screen.height, 1)
  const isPortrait = aspect < 0.72
  const width = viewport.width * (isPortrait ? 0.42 : aspect >= 1.55 ? 0.2 : 0.26)
  const height = width * (isPortrait ? 1.18 : 1.32)
  const slot = width * (isPortrait ? 1.28 : 1.08)

  return { width, height, slot }
}

export function getFanCardPose(x: number, viewportWidth: number, viewportHeight = viewportWidth): FanCardPose {
  const isPortrait = viewportHeight > viewportWidth * 1.35
  const normalized = x / Math.max(viewportWidth * 0.5, 1)
  const clamped = Math.max(-1.4, Math.min(1.4, normalized))
  const baseY = isPortrait ? -0.86 : -0.72
  const arcLift = isPortrait ? 0.28 : 0.62
  const rotationAmount = isPortrait ? 0.045 : 0.14

  return {
    x,
    y: baseY + Math.cos(clamped * Math.PI * 0.5) * arcLift,
    rotationZ: -clamped * rotationAmount,
  }
}

export function getActiveFanIndex(xs: number[]): number {
  if (xs.length === 0) return -1

  let active = 0
  let nearest = Math.abs(xs[0])
  for (let i = 1; i < xs.length; i++) {
    const distance = Math.abs(xs[i])
    if (distance < nearest) {
      active = i
      nearest = distance
    }
  }

  return active
}

export function getFanPosterStats(counts: number[]) {
  return {
    albumCount: counts.length,
    photoCount: counts.reduce((sum, count) => sum + count, 0),
  }
}

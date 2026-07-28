export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t
}

export function map(
  value: number,
  inMin: number,
  inMax: number,
  outMin: number,
  outMax: number,
): number {
  return ((value - inMin) / (inMax - inMin)) * (outMax - outMin) + outMin
}

export function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value))
}

export function wrap(value: number, min: number, max: number): number {
  const range = max - min
  return min + ((((value - min) % range) + range) % range)
}

export function degToRad(deg: number): number {
  return deg * (Math.PI / 180)
}

export function radToDeg(rad: number): number {
  return rad * (180 / Math.PI)
}

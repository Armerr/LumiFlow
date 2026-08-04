import * as THREE from 'three'
import { describe, expect, test } from 'vitest'
import { getActiveFanIndex, getFanAlbumAtScreenPoint, getFanCardMetrics, getFanCardPose, getFanPosterStats, isFanDrag, isFanGestureStart } from './fanLayout'

describe('fan layout', () => {
  test('sizes album cards so multiple cards fit in the viewport', () => {
    const metrics = getFanCardMetrics(
      { width: 1440, height: 900 },
      { width: 26.5, height: 16.56 },
    )

    expect(metrics.width).toBeLessThan(26.5 / 4)
    expect(metrics.height).toBeLessThan(16.56 * 0.55)
    expect(metrics.slot).toBeGreaterThan(metrics.width)
  })

  test('keeps circular-gallery arc positions inside the visible camera range', () => {
    const center = getFanCardPose(0, 26.5)
    const left = getFanCardPose(-10, 26.5)
    const right = getFanCardPose(10, 26.5)

    expect(center.y).toBeGreaterThan(-1)
    expect(left.y).toBeGreaterThan(-2)
    expect(right.y).toBeGreaterThan(-2)
    expect(Math.abs(left.rotationZ)).toBeLessThan(0.25)
    expect(Math.abs(right.rotationZ)).toBeLessThan(0.25)
  })

  test('uses a non-overlapping shelf on portrait mobile', () => {
    const metrics = getFanCardMetrics(
      { width: 390, height: 844 },
      { width: 7.66, height: 16.56 },
    )

    expect(metrics.slot).toBeGreaterThan(metrics.width * 0.7)
    expect(metrics.width).toBeGreaterThan(7.66 * 0.4)
  })

  test('places portrait mobile cards in a compact mid-screen shelf', () => {
    const center = getFanCardPose(0, 7.66, 16.56)
    const side = getFanCardPose(3.8, 7.66, 16.56)

    expect(center.y).toBeGreaterThan(-0.9)
    expect(center.y).toBeLessThan(-0.15)
    expect(Math.abs(side.rotationZ)).toBeLessThan(0.06)
  })

  test('selects the card closest to screen center as active', () => {
    expect(getActiveFanIndex([6, -1.2, 2.4, 0.35])).toBe(3)
    expect(getActiveFanIndex([])).toBe(-1)
  })

  test('chooses a WebGL card under a tap without a DOM overlay', () => {
    const album = { id: 'day:2026-07-28', name: '2026.07.28 · 东京' }
    const camera = new THREE.PerspectiveCamera(45, 1000 / 800, 0.1, 100)
    camera.position.z = 20
    camera.lookAt(0, 0, 0)
    camera.updateProjectionMatrix()
    camera.updateMatrixWorld()

    const mesh = new THREE.Mesh(new THREE.PlaneGeometry(4, 6), new THREE.MeshBasicMaterial())
    mesh.updateMatrixWorld()
    expect(getFanAlbumAtScreenPoint(camera, [{ album, mesh }], { left: 0, top: 0, width: 1000, height: 800 }, 500, 400)).toBe(album)
    expect(getFanAlbumAtScreenPoint(camera, [{ album, mesh }], { left: 0, top: 0, width: 1000, height: 800 }, 20, 20)).toBeUndefined()

    mesh.geometry.dispose()
    mesh.material.dispose()
  })

  test('treats horizontal displacement past six pixels as a drag', () => {
    expect(isFanDrag(500, 506)).toBe(false)
    expect(isFanDrag(500, 507)).toBe(true)
  })
  test('tracks initiating pointer so second finger panning does not classify a drag', () => {
    expect(isFanGestureStart(1)).toBe(true)
    expect(isFanGestureStart(1, 1)).toBe(true)
    expect(isFanGestureStart(1, 2)).toBe(false)
  })
  test('summarizes albums for poster chrome', () => {
    expect(getFanPosterStats([6, 8, 10])).toEqual({ albumCount: 3, photoCount: 24 })
    expect(getFanPosterStats([])).toEqual({ albumCount: 0, photoCount: 0 })
  })
})
